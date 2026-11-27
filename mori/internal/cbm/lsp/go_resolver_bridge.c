/* Native parsing, arena, and registry storage for the Rust Go resolver. */
#include "go_lsp.h"
#include "../native_tree_ffi.h"
#include "lsp_neg_memo.h"
#include "lsp_work.h"
#include "../../../src/foundation/mem_core.h"
#include "../helpers.h"
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>

// GoLSPContext holds state for Go expression type evaluation within a file.
typedef struct {
    CBMArena* arena;
    const char* source;
    int source_len;
    const CBMTypeRegistry* registry;
    CBMScope* current_scope;

    // Import map: local_name -> package QN (arena-allocated)
    const char** import_local_names;   // NULL-terminated
    const char** import_package_qns;   // NULL-terminated
    int import_count;
    CBMNameRows import_rows; // import_local_names by name, rows in table order

    // Current function context
    const char* enclosing_func_qn;
    const char* package_qn;  // module QN for this file
    const char* rel_path;    // repo-relative file path: init() QN suffix (#1910); NULL = flat

    // Output: resolved calls accumulate here
    CBMResolvedCallArray *resolved_calls;
    const _Atomic int *cancelled;

} GoLSPContext;

static void extract_type_params_from_ast(CBMArena *arena, CBMTypeRegistry *reg, TSNode root,
                                         const char *source, int source_len, const char *module_qn);

// --- Initialization ---

static void go_lsp_init(GoLSPContext *ctx, CBMArena *arena, const char *source, int source_len,
                 const CBMTypeRegistry *registry, const char *package_qn,
                 CBMResolvedCallArray *out) {
    memset(ctx, 0, sizeof(GoLSPContext));
    ctx->arena = arena;
    ctx->source = source;
    ctx->source_len = source_len;
    ctx->registry = registry;
    ctx->package_qn = package_qn;
    ctx->resolved_calls = out;
    ctx->current_scope = cbm_scope_push(arena, NULL); // root scope

}

static void go_lsp_add_import(GoLSPContext *ctx, const char *local_name, const char *pkg_qn) {
    // Store in parallel arrays (arena-allocated, grow by doubling)
    if (ctx->import_count % 32 == 0) {
        int new_cap = ctx->import_count + 32;
        const char **new_names =
            (const char **)cbm_arena_alloc(ctx->arena, (new_cap + 1) * sizeof(const char *));
        const char **new_qns =
            (const char **)cbm_arena_alloc(ctx->arena, (new_cap + 1) * sizeof(const char *));
        if (!new_names || !new_qns)
            return;
        if (ctx->import_local_names && ctx->import_count > 0) {
            memcpy(new_names, ctx->import_local_names, ctx->import_count * sizeof(const char *));
            memcpy(new_qns, ctx->import_package_qns, ctx->import_count * sizeof(const char *));
        }
        ctx->import_local_names = new_names;
        ctx->import_package_qns = new_qns;
    }
    ctx->import_local_names[ctx->import_count] = cbm_arena_strdup(ctx->arena, local_name);
    ctx->import_package_qns[ctx->import_count] = cbm_arena_strdup(ctx->arena, pkg_qn);
    ctx->import_count++;
}

// --- Helper: resolve import alias to package QN ---

/* The first import of this local name. Found by name (lsp_name_rows.h): a scan
 * per use made a file of thousands of imports quadratic. */
static const char *resolve_import(GoLSPContext *ctx, const char *local_name) {
    if (!local_name)
        return NULL;
    int i = cbm_name_rows_first(&ctx->import_rows, ctx->arena, ctx->import_local_names,
                                ctx->import_count, local_name);
    return i >= 0 ? ctx->import_package_qns[i] : NULL;
}

// --- go_parse_type_node: AST type node -> CBMType ---

/* The Rust parser borrows the pinned tree and emits arena-owned CBMTypes. */
typedef struct {
    const unsigned char *data;
    size_t len;
} GoTypeBytes;

typedef enum {
    GO_TYPE_UNKNOWN,
    GO_TYPE_BUILTIN,
    GO_TYPE_NAMED,
    GO_TYPE_POINTER,
    GO_TYPE_SLICE,
    GO_TYPE_MAP,
    GO_TYPE_CHANNEL,
    GO_TYPE_FUNCTION,
    GO_TYPE_INTERFACE,
    GO_TYPE_STRUCT,
    GO_TYPE_TUPLE,
    GO_TYPE_PARAMETER,
} GoTypeKind;

typedef const char *(*GoTypeImport)(void *, GoTypeBytes);
typedef const CBMType *(*GoTypeMake)(void *, GoTypeKind, GoTypeBytes, GoTypeBytes,
                                     const CBMType **, size_t, int);
extern const CBMType *mori_go_parse_type(TSNode, GoTypeBytes, const char *, void *,
                                         GoTypeImport, const char *const *, GoTypeMake);

static const char *go_type_import(void *opaque, GoTypeBytes alias) {
    GoLSPContext *ctx = opaque;
    return resolve_import(ctx, cbm_arena_strndup(ctx->arena, (const char *)alias.data, alias.len));
}

static const CBMType *go_type_make(void *opaque, GoTypeKind kind, GoTypeBytes package,
                                   GoTypeBytes name, const CBMType **children, size_t count,
                                   int detail) {
    CBMArena *arena = ((GoLSPContext *)opaque)->arena;
    /* Rust writes the child array before passing it to the native constructors. */
    if (count) {
        CBM_MSAN_WRITTEN(children, count * sizeof(*children));
    }
    switch (kind) {
    case GO_TYPE_BUILTIN:
        return cbm_type_builtin(arena, cbm_arena_strndup(arena, (const char *)name.data, name.len));
    case GO_TYPE_NAMED: {
        char *qualified = cbm_arena_alloc(arena, package.len + name.len + 2);
        if (!qualified)
            return cbm_type_unknown();
        memcpy(qualified, package.data, package.len);
        qualified[package.len] = '.';
        memcpy(qualified + package.len + 1, name.data, name.len);
        qualified[package.len + name.len + 1] = '\0';
        return cbm_type_named(arena, qualified);
    }
    case GO_TYPE_POINTER:
        return cbm_type_pointer(arena, children[0]);
    case GO_TYPE_SLICE:
        return cbm_type_slice(arena, children[0]);
    case GO_TYPE_MAP:
        return cbm_type_map(arena, children[0], children[1]);
    case GO_TYPE_CHANNEL:
        return cbm_type_channel(arena, children[0], detail);
    case GO_TYPE_FUNCTION: {
        enum { MAX_SIGNATURE_TYPES = 15 };
        size_t param_count = (size_t)detail;
        if (param_count > count || param_count > MAX_SIGNATURE_TYPES ||
            count - param_count > MAX_SIGNATURE_TYPES)
            return cbm_type_unknown();
        size_t return_count = count - param_count;
        const CBMType *params[MAX_SIGNATURE_TYPES + 1];
        const CBMType *returns[MAX_SIGNATURE_TYPES + 1];
        for (size_t i = 0; i < param_count; i++)
            params[i] = children[i];
        for (size_t i = 0; i < return_count; i++)
            returns[i] = children[param_count + i];
        params[param_count] = NULL;
        returns[return_count] = NULL;
        return cbm_type_func(arena, NULL, param_count ? params : NULL,
                             return_count ? returns : NULL);
    }
    case GO_TYPE_PARAMETER:
        return cbm_type_type_param(
            arena, cbm_arena_strndup(arena, (const char *)name.data, name.len));
    case GO_TYPE_INTERFACE:
    case GO_TYPE_STRUCT: {
        CBMType *type = cbm_arena_alloc(arena, sizeof(*type));
        if (!type)
            return cbm_type_unknown();
        memset(type, 0, sizeof(*type));
        type->kind = kind == GO_TYPE_INTERFACE ? CBM_TYPE_INTERFACE : CBM_TYPE_STRUCT;
        return type;
    }
    case GO_TYPE_TUPLE:
        return cbm_type_tuple(arena, children, (int)count);
    case GO_TYPE_UNKNOWN:
        return cbm_type_unknown();
    }
    return cbm_type_unknown();
}

static const CBMType *go_parse_type_node(GoLSPContext *ctx, TSNode node) {
    if (ts_node_is_null(node) || !ctx->source || ctx->source_len < 0 || !ctx->package_qn)
        return cbm_type_unknown();
    GoTypeBytes source = {(const unsigned char *)ctx->source, (size_t)ctx->source_len};
    return mori_go_parse_type(node, source, ctx->package_qn, ctx, go_type_import, NULL,
                              go_type_make);
}

/* The Rust evaluator reads a prefix of the native type and function records. */
_Static_assert(sizeof(CBMTypeKind) == sizeof(uint32_t), "Go evaluator type kind width");
_Static_assert(CBM_TYPE_UNKNOWN == 0 && CBM_TYPE_NAMED == 1 && CBM_TYPE_POINTER == 2 &&
                   CBM_TYPE_SLICE == 3 && CBM_TYPE_MAP == 4 && CBM_TYPE_CHANNEL == 5 &&
                   CBM_TYPE_FUNC == 6 && CBM_TYPE_INTERFACE == 7 && CBM_TYPE_BUILTIN == 9 && CBM_TYPE_TUPLE == 10 &&
                   CBM_TYPE_TYPE_PARAM == 11,
               "Go evaluator type tags");
_Static_assert(offsetof(CBMType, data) == sizeof(void *), "Go evaluator type payload");
_Static_assert(offsetof(CBMType, data.func.return_types) == 3 * sizeof(void *),
               "Go evaluator signature return types");
_Static_assert(offsetof(CBMRegisteredFunc, signature) == 3 * sizeof(void *),
               "Go evaluator function signature");
_Static_assert(offsetof(CBMRegisteredFunc, type_param_names) == 4 * sizeof(void *),
               "Go evaluator function type parameters");

_Static_assert(offsetof(CBMRegisteredType, method_names) == 4 * sizeof(void *),
               "Go resolver interface methods");
_Static_assert(offsetof(CBMRegisteredType, is_interface) == 9 * sizeof(void *),
               "Go resolver interface flag");
_Static_assert(CBM_RESOLVED_INVOCATION == 0 && CBM_RESOLVED_CALL_REFERENCE == 1,
               "Go resolver call kind");
_Static_assert(CBM_SOURCE_ORIGIN_RAW == 0, "Go resolver source origin");

typedef struct {
    const char *target;
    const char *strategy;
    const char *reason;
    float confidence;
    uint32_t kind;
    uint32_t start;
    uint32_t end;
    uint32_t origin;
} GoEvalOutput;

typedef struct {
    bool (*scope_lookup)(GoLSPContext *, const char *, const CBMType **);
    const char *(*import)(GoLSPContext *, const char *);
    const CBMType *(*parse_type)(GoLSPContext *, const TSNode *);
    bool (*push_scope)(GoLSPContext *, CBMScope **);
    void (*pop_scope)(GoLSPContext *, CBMScope *);
    bool (*bind)(GoLSPContext *, const char *, const CBMType *);
    bool (*bind_value)(GoLSPContext *, const char *, const CBMType *, const char *);
    void (*set_caller)(GoLSPContext *, const char *);
    const char *(*scope_callable)(GoLSPContext *, const char *);
    void (*clear_callable)(GoLSPContext *, const char *);
    bool (*emit)(GoLSPContext *, const GoEvalOutput *);
    bool (*cancelled)(GoLSPContext *);
} GoEvalOps;

typedef struct {
    TSNode root;
    const unsigned char *source;
    size_t source_len;
    const char *package;
    const char *path;
    CBMArena *arena;
    const CBMTypeRegistry *registry;
    GoLSPContext *context;
    const GoEvalOps *operations;
    size_t depth_limit;
    const char *const *import_names;
    const char *const *import_values;
    size_t import_count;
} GoEvalInput;

extern CBMGoResolveStatus mori_go_walk_file(const GoEvalInput *input);

static bool go_eval_scope_lookup(GoLSPContext *ctx, const char *name, const CBMType **out) {
    const CBMVarBinding *binding = cbm_scope_lookup_binding(ctx->current_scope, name);
    if (!binding)
        return false;
    *out = binding->type;
    return true;
}

static const CBMType *go_eval_parse_type(GoLSPContext *ctx, const TSNode *node) {
    cbm_ffi_ts_node_written(node);
    return go_parse_type_node(ctx, *node);
}

static bool go_eval_push_scope(GoLSPContext *ctx, CBMScope **parent) {
    *parent = ctx->current_scope;
    ctx->current_scope = cbm_scope_push(ctx->arena, *parent);
    return ctx->current_scope != *parent;
}

static void go_eval_pop_scope(GoLSPContext *ctx, CBMScope *parent) {
    ctx->current_scope = parent;
}

static bool go_eval_bind(GoLSPContext *ctx, const char *name, const CBMType *type) {
    return cbm_scope_bind_checked(ctx->current_scope, name, type);
}

static bool go_eval_bind_value(GoLSPContext *ctx, const char *name, const CBMType *type,
                               const char *callable) {
    return cbm_scope_bind_callable_checked(ctx->current_scope, name, type, callable);
}

static const char *go_eval_scope_callable(GoLSPContext *ctx, const char *name) {
    return cbm_scope_lookup_callable(ctx->current_scope, name);
}

static void go_eval_clear_callable(GoLSPContext *ctx, const char *name) {
    cbm_scope_update_callable(ctx->current_scope, name, NULL);
}

#ifdef CBM_ENABLE_TEST_SEAMS
static _Thread_local int go_test_rows_before_cancel = -1;

void cbm_go_test_cancel_walk_after(int rows) {
    go_test_rows_before_cancel = rows;
}
#endif

static bool go_eval_emit(GoLSPContext *ctx, const GoEvalOutput *row) {
    if (!ctx->enclosing_func_qn)
        return true;
    CBM_MSAN_WRITTEN(row, sizeof(*row));
    CBMResolvedCall call = {
        .caller_qn = ctx->enclosing_func_qn,
        .callee_qn = row->target,
        .strategy = row->strategy,
        .reason = row->reason,
        .confidence = row->confidence,
        .kind = (CBMResolvedKind)row->kind,
        .site_start_byte = row->start,
        .site_end_byte = row->end,
        .source_origin = (CBMSourceOrigin)row->origin,
    };
    int prior = ctx->resolved_calls->count;
    cbm_resolvedcall_push(ctx->resolved_calls, ctx->arena, call);
    bool emitted = ctx->resolved_calls->count > prior;
#ifdef CBM_ENABLE_TEST_SEAMS
    if (emitted && go_test_rows_before_cancel > 0)
        go_test_rows_before_cancel--;
#endif
    return emitted;
}

static bool go_token_cancelled(const _Atomic int *cancelled) {
    return cancelled && atomic_load_explicit(cancelled, memory_order_relaxed);
}

static bool go_eval_cancelled(GoLSPContext *ctx) {
#ifdef CBM_ENABLE_TEST_SEAMS
    if (go_test_rows_before_cancel == 0)
        return true;
#endif
    return go_token_cancelled(ctx->cancelled);
}

typedef bool (*GoEvalVisitType)(void *, const CBMRegisteredType *);

void cbm_go_eval_visit_types(const CBMTypeRegistry *reg, void *context, GoEvalVisitType visit) {
    CBMTypeShortIter types;
    cbm_registry_all_types_chain(reg, &types);
    for (int i = cbm_type_short_iter_next(&types); i >= 0; i = cbm_type_short_iter_next(&types)) {
        if (!visit(context, &types.reg->types[i]))
            break;
    }
}

extern char *mori_go_receiver_type_name(CBMArena *, const TSNode *, const unsigned char *,
                                        size_t);
extern const char *mori_go_init_qn(CBMArena *, const char *, const char *, const TSNode *,
                                   const unsigned char *, size_t);

char *cbm_go_receiver_type_name(CBMArena *arena, TSNode receiver, const char *source,
                                 int source_len) {
    if (!source || source_len < 0 || ts_node_is_null(receiver))
        return NULL;
    return mori_go_receiver_type_name(arena, &receiver, (const unsigned char *)source,
                                      (size_t)source_len);
}

const char *cbm_go_init_qn(CBMArena *arena, const char *base, const char *path, TSNode declaration,
                           const char *source, int source_len) {
    if (!base || !path || !source || source_len < 0 || ts_node_is_null(declaration))
        return base;
    return mori_go_init_qn(arena, base, path, &declaration, (const unsigned char *)source,
                           (size_t)source_len);
}

static void go_eval_set_caller(GoLSPContext *ctx, const char *caller) {
    ctx->enclosing_func_qn = caller;
}

static CBMGoResolveStatus go_lsp_process_file(GoLSPContext *ctx, TSNode node) {
    if (go_eval_cancelled(ctx))
        return CBM_GO_RESOLVE_CANCELLED;
    if (ts_node_is_null(node) || !ctx->source || ctx->source_len < 0 || !ctx->package_qn)
        return CBM_GO_RESOLVE_UNAVAILABLE;
    if (!ctx->current_scope || !ctx->resolved_calls)
        return CBM_GO_RESOLVE_FAILED;
    static const GoEvalOps ops = {
        .scope_lookup = go_eval_scope_lookup,
        .import = resolve_import,
        .parse_type = go_eval_parse_type,
        .push_scope = go_eval_push_scope,
        .pop_scope = go_eval_pop_scope,
        .bind = go_eval_bind,
        .bind_value = go_eval_bind_value,
        .set_caller = go_eval_set_caller,
        .scope_callable = go_eval_scope_callable,
        .clear_callable = go_eval_clear_callable,
        .emit = go_eval_emit,
        .cancelled = go_eval_cancelled,
    };
    GoEvalInput input = {
        .root = node,
        .source = (const unsigned char *)ctx->source,
        .source_len = (size_t)ctx->source_len,
        .package = ctx->package_qn,
        .path = ctx->rel_path,
        .arena = ctx->arena,
        .registry = ctx->registry,
        .context = ctx,
        .operations = &ops,
        .depth_limit = (size_t)cbm_lsp_max_walk_depth(),
        .import_names = ctx->import_local_names,
        .import_values = ctx->import_package_qns,
        .import_count = (size_t)ctx->import_count,
    };
    CBMResolvedCallArray *output = ctx->resolved_calls;
    CBMResolvedCallArray staged = *output;
    ctx->resolved_calls = &staged;
    CBMGoResolveStatus status = mori_go_walk_file(&input);
    ctx->resolved_calls = output;
    if (status == CBM_GO_RESOLVE_COMPLETE)
        *output = staged;
    return status;
}

/* Copy names and lists from Rust before native code consumes them. Counts
 * let MemorySanitizer mark only bytes written by the Rust evaluator. */
const char *cbm_go_eval_copy_name(CBMArena *arena, const unsigned char *data, size_t len) {
    CBM_MSAN_WRITTEN(data, len);
    return cbm_arena_strndup(arena, (const char *)data, len);
}

const CBMType *cbm_go_eval_func(CBMArena *arena, const CBMType *const *params, size_t param_count,
                                const CBMType *const *returns, size_t return_count) {
    enum { MAX_SIGNATURE_TYPES = 15 };
    if (param_count > MAX_SIGNATURE_TYPES || return_count > MAX_SIGNATURE_TYPES)
        return cbm_type_unknown();
    const CBMType *p[MAX_SIGNATURE_TYPES + 1];
    const CBMType *r[MAX_SIGNATURE_TYPES + 1];
    CBM_MSAN_WRITTEN(params, param_count * sizeof(*params));
    CBM_MSAN_WRITTEN(returns, return_count * sizeof(*returns));
    memcpy(p, params, param_count * sizeof(*params));
    memcpy(r, returns, return_count * sizeof(*returns));
    p[param_count] = NULL;
    r[return_count] = NULL;
    return cbm_type_func(arena, NULL, param_count ? p : NULL, return_count ? r : NULL);
}

const CBMType *cbm_go_eval_tuple(CBMArena *arena, const CBMType **types, int count) {
    CBM_MSAN_WRITTEN(types, (size_t)count * sizeof(*types));
    return cbm_type_tuple(arena, types, count);
}

const CBMType *cbm_go_eval_substitute(CBMArena *arena, const CBMType *type, const char **names,
                                      const CBMType *const *args, size_t count) {
    enum { MAX_GO_TYPE_ARGS = 16 };
    if (count > MAX_GO_TYPE_ARGS)
        return type;
    const CBMType *values[MAX_GO_TYPE_ARGS + 1];
    CBM_MSAN_WRITTEN(args, count * sizeof(*args));
    memcpy(values, args, count * sizeof(*args));
    values[count] = NULL;
    return cbm_type_substitute(arena, type, names, values);
}

// --- Shared Go stdlib registry ---

/* The Go stdlib (2,328 functions, 321 types) registered ONCE per process into
 * a finalized, sealed registry that every per-file registry chains to as its
 * fallback -- the overlay contract of type_registry.h: lookups and the chain
 * iterators reach it, refinements copy into the head, nothing writes it.
 * Registered into each file's registry instead, it was 13.4 k re-registrations
 * on the Go corpus, 2.5 GB of copies, and every lookup of the unfinalized
 * per-file registry scanned the stdlib linearly (waste sanitizer, 2026-09-17).
 * Threads racing the first build each build a copy and the first to publish
 * wins; the others free theirs -- no thread waits. NULL on allocation
 * failure: the caller registers the stdlib into its own registry as before. */
typedef struct {
    CBMArena arena;
    CBMTypeRegistry reg;
} go_stdlib_holder_t;

static _Atomic(go_stdlib_holder_t *) g_go_stdlib_shared;

static const CBMTypeRegistry *go_stdlib_shared(void) {
    go_stdlib_holder_t *ready = atomic_load_explicit(&g_go_stdlib_shared, memory_order_acquire);
    if (ready)
        return &ready->reg;
    go_stdlib_holder_t *built = cbm_calloc(CBM_MEM_CLASS_OTHER, sizeof(*built));
    if (!built)
        return NULL;
    cbm_arena_init(&built->arena);
    if (built->arena.nblocks == 0) {
        cbm_free(CBM_MEM_CLASS_OTHER, built);
        return NULL;
    }
    cbm_registry_init(&built->reg, &built->arena);
    cbm_go_stdlib_register(&built->reg, &built->arena);
    cbm_registry_finalize(&built->reg);
    built->reg.read_only = true;
    go_stdlib_holder_t *expected = NULL;
    if (atomic_compare_exchange_strong_explicit(&g_go_stdlib_shared, &expected, built,
                                                memory_order_acq_rel, memory_order_acquire)) {
        return &built->reg;
    }
    cbm_arena_destroy(&built->arena);
    cbm_free(CBM_MEM_CLASS_OTHER, built);
    return &expected->reg;
}

/* A per-file registry sees the stdlib: chained to the shared copy, or
 * registered into it when the shared copy cannot be built. */
static void go_registry_with_stdlib(CBMTypeRegistry *reg, CBMArena *arena) {
    const CBMTypeRegistry *shared = go_stdlib_shared();
    if (shared) {
        reg->fallback = shared;
    } else {
        cbm_go_stdlib_register(reg, arena);
    }
}

/* The first row of each type QN in a filling registry, as a scan from the top
 * finds it. Registering and refining a file's types asked a scan of every type
 * per definition, and a generated file holds thousands. A row the memo cannot
 * take (no memory) sends every later question back to the scan. */
enum { GO_MEMO_ARENA_BLOCK = 4096 };

typedef struct {
    CBMIdxMemo first;
    CBMArena *arena;
    bool failed;
} GoTypeRows;

static void go_type_rows_note(GoTypeRows *rows, const CBMTypeRegistry *reg, int row) {
    const char *qn = reg->types[row].qualified_name;
    if (rows->failed || !qn)
        return;
    CBM_LSP_WORK(1);
    cbm_idxmemo_put_if_absent(&rows->first, rows->arena, qn, row);
    if (cbm_idxmemo_get(&rows->first, qn) < 0)
        rows->failed = true;
}

static void go_type_rows_add(CBMTypeRegistry *reg, GoTypeRows *rows, CBMRegisteredType type) {
    int before = reg->type_count;
    cbm_registry_add_type(reg, type);
    if (reg->type_count > before)
        go_type_rows_note(rows, reg, reg->type_count - 1);
}

static int go_type_rows_first(const GoTypeRows *rows, const CBMTypeRegistry *reg, const char *qn) {
    CBM_LSP_WORK(1);
    if (!rows->failed)
        return cbm_idxmemo_get(&rows->first, qn);
    for (int ti = 0; ti < reg->type_count; ti++) {
        CBM_LSP_WORK(1);
        if (reg->types[ti].qualified_name && strcmp(reg->types[ti].qualified_name, qn) == 0)
            return ti;
    }
    return -1;
}

typedef struct {
    GoLSPContext *file;
    CBMTypeRegistry *registry;
    GoTypeRows *own_types;
} GoAstRegistry;

typedef enum {
    GO_AST_EMBEDS,
    GO_AST_METHODS,
} GoAstNameRows;

typedef struct {
    const char *(*name)(GoAstRegistry *, const unsigned char *, size_t);
    int (*find_type)(GoAstRegistry *, const char *);
    void (*alias)(GoAstRegistry *, int, const char *, const char *, const char *);
    void (*names)(GoAstRegistry *, int, GoAstNameRows, const char *const *, size_t);
    void (*fields)(GoAstRegistry *, int, const char *const *, const CBMType *const *, size_t);
    const CBMType *(*parse_type)(GoAstRegistry *, const TSNode *);
} GoAstRegistryOps;

typedef struct {
    TSNode root;
    const unsigned char *source;
    size_t source_len;
    const char *package;
    GoAstRegistry *context;
    const GoAstRegistryOps *ops;
    bool refine_interfaces;
} GoAstRegistryInput;

extern void mori_go_refine_registry_ast(const GoAstRegistryInput *);

static const char *go_ast_name(GoAstRegistry *ctx, const unsigned char *bytes, size_t len) {
    return cbm_go_eval_copy_name(ctx->file->arena, bytes, len);
}

static int go_ast_find_type(GoAstRegistry *ctx, const char *name) {
    if (ctx->own_types)
        return go_type_rows_first(ctx->own_types, ctx->registry, name);
    for (int i = 0; i < ctx->registry->type_count; i++) {
        const char *qn = ctx->registry->types[i].qualified_name;
        if (qn && strcmp(qn, name) == 0)
            return i;
    }
    return -1;
}

static void go_ast_alias(GoAstRegistry *ctx, int row, const char *name, const char *qn,
                         const char *target) {
    if (row >= 0) {
        ctx->registry->types[row].alias_of = target;
        return;
    }
    CBMRegisteredType type = {.qualified_name = qn, .short_name = name, .alias_of = target};
    if (ctx->own_types)
        go_type_rows_add(ctx->registry, ctx->own_types, type);
    else
        cbm_registry_add_type(ctx->registry, type);
}

static const char **go_ast_copy_names(CBMArena *arena, const char *const *names, size_t count) {
    const char **copy = cbm_arena_alloc(arena, (count + 1) * sizeof(*copy));
    if (copy) {
        CBM_MSAN_WRITTEN(names, count * sizeof(*names));
        memcpy(copy, names, count * sizeof(*names));
        copy[count] = NULL;
    }
    return copy;
}

static void go_ast_names(GoAstRegistry *ctx, int row, GoAstNameRows kind,
                         const char *const *names, size_t count) {
    const char **copy = go_ast_copy_names(ctx->file->arena, names, count);
    if (copy) {
        CBMRegisteredType *type = &ctx->registry->types[row];
        if (kind == GO_AST_EMBEDS)
            type->embedded_types = copy;
        else
            type->method_names = copy;
    }
}

static void go_ast_fields(GoAstRegistry *ctx, int row, const char *const *names,
                          const CBMType *const *types, size_t count) {
    CBMArena *arena = ctx->file->arena;
    const char **field_names = go_ast_copy_names(arena, names, count);
    const CBMType **field_types = cbm_arena_alloc(arena, (count + 1) * sizeof(*field_types));
    if (field_names && field_types) {
        CBM_MSAN_WRITTEN(types, count * sizeof(*types));
        memcpy(field_types, types, count * sizeof(*types));
        field_types[count] = NULL;
        ctx->registry->types[row].field_names = field_names;
        ctx->registry->types[row].field_types = field_types;
    }
}

static const CBMType *go_ast_parse_type(GoAstRegistry *ctx, const TSNode *node) {
    return go_eval_parse_type(ctx->file, node);
}

static void go_refine_registry_ast(GoAstRegistry ctx, TSNode root, bool refine_interfaces) {
    if (ts_node_is_null(root) || !ctx.file->source || ctx.file->source_len < 0 ||
        !ctx.file->package_qn)
        return;
    static const GoAstRegistryOps ops = {
        .name = go_ast_name,
        .find_type = go_ast_find_type,
        .alias = go_ast_alias,
        .names = go_ast_names,
        .fields = go_ast_fields,
        .parse_type = go_ast_parse_type,
    };
    GoAstRegistryInput input = {
        .root = root,
        .source = (const unsigned char *)ctx.file->source,
        .source_len = (size_t)ctx.file->source_len,
        .package = ctx.file->package_qn,
        .context = &ctx,
        .ops = &ops,
        .refine_interfaces = refine_interfaces,
    };
    mori_go_refine_registry_ast(&input);
}

typedef enum {
    GO_DEFS_LOCAL,
    GO_DEFS_FILTERED,
    GO_DEFS_PROJECT,
} GoDefinitionMode;

typedef struct {
    const char *qualified;
    const char *name;
    const char *label;
    const char *module;
    const char *receiver_qn;
    const char *receiver_spelling;
    const char *const *return_types;
    const char *return_text;
    const char *const *ordered_params;
    int ordered_count;
    const char *const *legacy_params;
    const char *const *parameter_names;
    const char *const *base_classes;
    const char *embedded_names;
    const char *field_defs;
    const char *method_names;
    const char *alias_target;
    bool interface;
} GoDefinition;

typedef struct {
    const char *qualified;
    const char *name;
    const char *alias_of;
    const char *const *embeds;
    size_t embed_count;
    const char *const *methods;
    size_t method_count;
    bool interface;
} GoTypeInput;

typedef struct {
    const char *qualified;
    const char *name;
    const char *receiver;
    const char **parameter_names;
    const CBMType *const *parameters;
    size_t parameter_count;
    const CBMType *const *returns;
    size_t return_count;
} GoFunctionInput;

typedef struct {
    GoAstRegistry ast;
    const void *definitions;
    GoDefinitionMode mode;
} GoDefRegistry;

typedef struct {
    bool (*read)(GoDefRegistry *, size_t, GoDefinition *);
    bool (*has_type)(GoDefRegistry *, const char *);
    void (*add_type)(GoDefRegistry *, const GoTypeInput *);
    void (*fields)(GoDefRegistry *, const char *, const char *const *, const CBMType *const *,
                   size_t);
    void (*add_function)(GoDefRegistry *, const GoFunctionInput *);
} GoDefRegistryOps;

extern void mori_go_register_definitions(GoDefRegistry *, CBMArena *, const GoDefRegistryOps *,
                                         size_t, const char *, GoDefinitionMode);

static bool go_def_read(GoDefRegistry *ctx, size_t index, GoDefinition *out) {
    if (ctx->mode == GO_DEFS_LOCAL) {
        const CBMDefinition *def = &((const CBMDefinition *)ctx->definitions)[index];
        *out = (GoDefinition){
            .qualified = def->qualified_name,
            .name = def->name,
            .label = def->label,
            .receiver_qn = def->parent_class,
            .receiver_spelling = def->receiver,
            .return_types = def->return_types,
            .return_text = def->return_type,
            .ordered_params = def->signature_param_types,
            .ordered_count = def->signature_param_count,
            .legacy_params = def->param_types,
            .parameter_names = def->param_names,
            .base_classes = def->base_classes,
            .interface = def->label && strcmp(def->label, "Interface") == 0,
        };
        return true;
    }
    const CBMLSPDef *def = &((const CBMLSPDef *)ctx->definitions)[index];
    if (ctx->mode == GO_DEFS_PROJECT && def->lang != CBM_LANG_GO)
        return false;
    *out = (GoDefinition){
        .qualified = def->qualified_name,
        .name = def->short_name,
        .label = def->label,
        .module = def->def_module_qn,
        .receiver_qn = def->receiver_type,
        .return_text = def->return_types,
        .ordered_params = def->signature_param_types,
        .ordered_count = def->signature_param_count,
        .embedded_names = def->embedded_types,
        .field_defs = def->field_defs,
        .method_names = def->method_names_str,
        .alias_target = def->alias_target,
        .interface = def->is_interface ||
                     (def->label && strcmp(def->label, "Interface") == 0),
    };
    return true;
}

static bool go_def_has_type(GoDefRegistry *ctx, const char *name) {
    return go_ast_find_type(&ctx->ast, name) >= 0 ||
           cbm_registry_lookup_type(ctx->ast.registry->fallback, name) != NULL;
}

static void go_def_add_type(GoDefRegistry *ctx, const GoTypeInput *row) {
    CBM_MSAN_WRITTEN(row, sizeof(*row));
    CBMArena *arena = ctx->ast.file->arena;
    CBMRegisteredType type = {
        .qualified_name = row->qualified,
        .short_name = row->name,
        .alias_of = row->alias_of,
        .is_interface = row->interface,
        .embedded_types = row->embed_count ? go_ast_copy_names(arena, row->embeds, row->embed_count)
                                          : NULL,
        .method_names = row->method_count ? go_ast_copy_names(arena, row->methods, row->method_count)
                                          : NULL,
    };
    go_type_rows_add(ctx->ast.registry, ctx->ast.own_types, type);
}

static void go_def_fields(GoDefRegistry *ctx, const char *name, const char *const *names,
                          const CBMType *const *types, size_t count) {
    int row = go_ast_find_type(&ctx->ast, name);
    if (row >= 0)
        go_ast_fields(&ctx->ast, row, names, types, count);
}

static const CBMType **go_def_copy_types(CBMArena *arena, const CBMType *const *types,
                                         size_t count) {
    if (!count)
        return NULL;
    const CBMType **copy = cbm_arena_alloc(arena, (count + 1) * sizeof(*copy));
    if (copy) {
        CBM_MSAN_WRITTEN(types, count * sizeof(*types));
        memcpy(copy, types, count * sizeof(*types));
        copy[count] = NULL;
    }
    return copy;
}

static void go_def_add_function(GoDefRegistry *ctx, const GoFunctionInput *row) {
    CBM_MSAN_WRITTEN(row, sizeof(*row));
    CBMArena *arena = ctx->ast.file->arena;
    const CBMType **parameters = go_def_copy_types(arena, row->parameters, row->parameter_count);
    const CBMType **returns = go_def_copy_types(arena, row->returns, row->return_count);
    CBMRegisteredFunc function = {
        .qualified_name = row->qualified,
        .short_name = row->name,
        .receiver_type = row->receiver,
        .signature = cbm_type_func(arena, row->parameter_names, parameters, returns),
    };
    cbm_registry_add_func(ctx->ast.registry, function);
}

static void go_register_definitions(GoAstRegistry ast, const void *defs, int count,
                                    GoDefinitionMode mode) {
    if (!defs || count <= 0)
        return;
    for (int i = 0; i < ast.registry->type_count; i++)
        go_type_rows_note(ast.own_types, ast.registry, i);
    static const GoDefRegistryOps ops = {
        .read = go_def_read,
        .has_type = go_def_has_type,
        .add_type = go_def_add_type,
        .fields = go_def_fields,
        .add_function = go_def_add_function,
    };
    GoDefRegistry ctx = {ast, defs, mode};
    mori_go_register_definitions(&ctx, ast.file->arena, &ops, (size_t)count,
                                  ast.file->package_qn, mode);
}

// --- Entry point: build registry from file defs + run LSP ---

CBMGoResolveStatus cbm_run_go_lsp_status(
    CBMArena *arena, CBMFileResult *result, const char *source, int source_len, TSNode root,
    const char *rel_path, const _Atomic int *cancelled) {
    if (go_token_cancelled(cancelled))
        return CBM_GO_RESOLVE_CANCELLED;
    if (!arena || !result || !source || source_len < 0 || ts_node_is_null(root))
        return CBM_GO_RESOLVE_UNAVAILABLE;
    CBMTypeRegistry reg;
    cbm_registry_init(&reg, arena);

    // Go stdlib types/functions: the shared sealed registry, chained
    go_registry_with_stdlib(&reg, arena);

    CBMArena memo_arena;
    cbm_arena_init_lazy(&memo_arena, GO_MEMO_ARENA_BLOCK);
    GoTypeRows own_types = {.arena = &memo_arena};
    const char *module_qn = result->module_qn;

    GoLSPContext ast = {
        .arena = arena,
        .source = source,
        .source_len = source_len,
        .registry = &reg,
        .package_qn = module_qn,
    };
    GoAstRegistry registry_ast = {&ast, &reg, &own_types};
    go_register_definitions(registry_ast, result->defs.items, result->defs.count, GO_DEFS_LOCAL);
    go_refine_registry_ast(registry_ast, root, true);
    cbm_arena_destroy(&memo_arena);

    // Phase 1c: Extract type parameters from generic function declarations
    extract_type_params_from_ast(arena, &reg, root, source, source_len, module_qn);

    // Phase 2: Build LSP context and run
    GoLSPContext lsp_ctx;
    go_lsp_init(&lsp_ctx, arena, source, source_len, &reg, module_qn, &result->resolved_calls);
    lsp_ctx.rel_path = rel_path;
    lsp_ctx.cancelled = cancelled;

    // Add imports
    for (int i = 0; i < result->imports.count; i++) {
        CBMImport *imp = &result->imports.items[i];
        if (imp->local_name && imp->module_path) {
            go_lsp_add_import(&lsp_ctx, imp->local_name, imp->module_path);
        }
    }

    // Process the file
    CBMGoResolveStatus status = go_lsp_process_file(&lsp_ctx, root);
    if (status == CBM_GO_RESOLVE_FAILED) {
        result->has_error = true;
        result->error_msg = CBM_GO_RESOLVE_ERROR;
    }
    return status;
}

void cbm_run_go_lsp(CBMArena *arena, CBMFileResult *result, const char *source, int source_len,
                    TSNode root, const char *rel_path) {
    (void)cbm_run_go_lsp_status(arena, result, source, source_len, root, rel_path, NULL);
}

// --- Phase 1c: Extract type parameters from generic function declarations ---

/* The first registration of each function QN, listed when the first generic
 * function asks: each one scanned every registered function for its own. The
 * functions do not change while they are patched. */
typedef struct {
    CBMIdxMemo first;
    CBMArena arena;
    bool listed;
    bool failed; /* a row could not be remembered: scan instead */
} GoFuncRows;

static int go_func_rows_first(GoFuncRows *rows, const CBMTypeRegistry *reg, const char *qn) {
    for (int fi = 0; !rows->listed && !rows->failed && fi < reg->func_count; fi++) {
        const char *name = reg->funcs[fi].qualified_name;
        if (!name)
            continue;
        CBM_LSP_WORK(1);
        cbm_idxmemo_put_if_absent(&rows->first, &rows->arena, name, fi);
        if (cbm_idxmemo_get(&rows->first, name) < 0)
            rows->failed = true;
    }
    rows->listed = true;
    CBM_LSP_WORK(1);
    if (!rows->failed)
        return cbm_idxmemo_get(&rows->first, qn);
    for (int fi = 0; fi < reg->func_count; fi++) {
        CBM_LSP_WORK(1);
        if (reg->funcs[fi].qualified_name && strcmp(reg->funcs[fi].qualified_name, qn) == 0)
            return fi;
    }
    return -1;
}

typedef struct {
    GoFuncRows first;
    CBMTypeRegistry *reg;
    CBMArena *arena;
    const char *source;
    int source_len;
    const char *module;
} GoGenericRegistry;

typedef struct {
    int row;
    const CBMType *signature;
} GoGenericFunction;

typedef struct {
    const char *const *names;
    size_t name_count;
    const CBMType *const *params;
    size_t param_count;
    const CBMType *const *returns;
    size_t return_count;
} GoGenericReplacement;

typedef struct {
    GoGenericFunction (*find)(GoGenericRegistry *, const char *);
    const CBMType *(*parse)(GoGenericRegistry *, const TSNode *, const char *const *, size_t);
    void (*set)(GoGenericRegistry *, int, const GoGenericReplacement *);
} GoGenericOps;

typedef struct {
    TSNode root;
    const unsigned char *source;
    size_t source_len;
    const char *module;
    CBMArena *arena;
    GoGenericRegistry *context;
    const GoGenericOps *ops;
} GoGenericInput;

extern void mori_go_refine_generics(const GoGenericInput *);

static GoGenericFunction go_generic_find(GoGenericRegistry *ctx, const char *name) {
    int row = go_func_rows_first(&ctx->first, ctx->reg, name);
    return (GoGenericFunction){row, row >= 0 ? ctx->reg->funcs[row].signature : NULL};
}

static const CBMType *go_generic_parse(GoGenericRegistry *ctx, const TSNode *node,
                                       const char *const *params, size_t count) {
    enum { MAX_TYPE_PARAMS = 15 };
    if (count > MAX_TYPE_PARAMS)
        return cbm_type_unknown();
    const char *names[MAX_TYPE_PARAMS + 1];
    CBM_MSAN_WRITTEN(params, count * sizeof(*params));
    memcpy(names, params, count * sizeof(*params));
    names[count] = NULL;

    cbm_ffi_ts_node_written(node);
    GoLSPContext file = {.arena = ctx->arena};
    GoTypeBytes source = {(const unsigned char *)ctx->source, (size_t)ctx->source_len};
    return mori_go_parse_type(*node, source, ctx->module, &file, NULL, names, go_type_make);
}

static void go_generic_set(GoGenericRegistry *ctx, int row, const GoGenericReplacement *refined) {
    CBM_MSAN_WRITTEN(refined, sizeof(*refined));
    CBMRegisteredFunc *function = &ctx->reg->funcs[row];
    const char **names = go_ast_copy_names(ctx->arena, refined->names, refined->name_count);
    if (names)
        function->type_param_names = names;

    const CBMType *sig = function->signature;
    if (!sig || sig->kind != CBM_TYPE_FUNC || (!refined->params && !refined->returns))
        return;

    const CBMType **params = sig->data.func.param_types;
    const CBMType **returns = sig->data.func.return_types;
    if (refined->params) {
        const CBMType **copy = go_def_copy_types(ctx->arena, refined->params, refined->param_count);
        if (copy)
            params = copy;
    }
    if (refined->returns) {
        const CBMType **copy = go_def_copy_types(ctx->arena, refined->returns, refined->return_count);
        if (copy)
            returns = copy;
    }
    const CBMType *replacement =
        cbm_type_func(ctx->arena, sig->data.func.param_names, params, returns);
    if (replacement && replacement->kind == CBM_TYPE_FUNC)
        function->signature = replacement;
}

static void extract_type_params_from_ast(CBMArena *arena, CBMTypeRegistry *reg, TSNode root,
                                         const char *source, int source_len, const char *module_qn) {
    if (ts_node_is_null(root) || !source || source_len < 0 || !module_qn)
        return;
    static const GoGenericOps ops = {
        .find = go_generic_find,
        .parse = go_generic_parse,
        .set = go_generic_set,
    };
    GoGenericRegistry ctx = {
        .reg = reg,
        .arena = arena,
        .source = source,
        .source_len = source_len,
        .module = module_qn,
    };
    cbm_arena_init_lazy(&ctx.first.arena, GO_MEMO_ARENA_BLOCK);
    GoGenericInput input = {
        .root = root,
        .source = (const unsigned char *)source,
        .source_len = (size_t)source_len,
        .module = module_qn,
        .arena = arena,
        .context = &ctx,
        .ops = &ops,
    };
    mori_go_refine_generics(&input);
    cbm_arena_destroy(&ctx.first.arena);
}

// Forward: tree-sitter Go language (from lang_specs.c)
extern const TSLanguage *tree_sitter_go(void);

CBMGoResolveStatus cbm_run_go_lsp_cross_status(
    CBMArena *arena, const char *source, int source_len, const char *module_qn,
    const char *rel_path, CBMLSPDef *defs, int def_count, const char **import_names,
    const char **import_qns, int import_count, TSTree *cached_tree, CBMResolvedCallArray *out,
    const _Atomic int *cancelled) {
    if (go_token_cancelled(cancelled))
        return CBM_GO_RESOLVE_CANCELLED;
    if (!source || source_len <= 0)
        return CBM_GO_RESOLVE_UNAVAILABLE;

    // 1. Use cached tree if available, otherwise parse fresh
    TSParser *parser = NULL;
    TSTree *tree = cached_tree;
    bool owns_tree = false;
    if (!tree) {
        parser = ts_parser_new();
        if (!parser)
            return CBM_GO_RESOLVE_FAILED;
        if (!ts_parser_set_language(parser, tree_sitter_go())) {
            ts_parser_delete(parser);
            return CBM_GO_RESOLVE_UNAVAILABLE;
        }
        tree = cbm_parse_source(parser, source, (uint32_t)source_len, (TSParseOptions){0});
        owns_tree = true;
        if (!tree) {
            ts_parser_delete(parser);
            return CBM_GO_RESOLVE_UNAVAILABLE;
        }
    }
    TSNode root = ts_tree_root_node(tree);

    // 2. Build registry (the stdlib is the shared sealed registry, chained)
    CBMTypeRegistry reg;
    cbm_registry_init(&reg, arena);
    go_registry_with_stdlib(&reg, arena);

    CBMArena memo_arena;
    cbm_arena_init_lazy(&memo_arena, GO_MEMO_ARENA_BLOCK);
    GoTypeRows own_types = {.arena = &memo_arena};

    GoLSPContext ast = {
        .arena = arena,
        .source = source,
        .source_len = source_len,
        .registry = &reg,
        .package_qn = module_qn,
        .import_local_names = import_names,
        .import_package_qns = import_qns,
        .import_count = import_count,
    };
    GoAstRegistry registry_ast = {&ast, &reg, &own_types};
    go_register_definitions(registry_ast, defs, def_count, GO_DEFS_FILTERED);
    go_refine_registry_ast(registry_ast, root, false);
    cbm_arena_destroy(&memo_arena);

    // 3b. Phase 1c: Extract type params from generic function declarations
    extract_type_params_from_ast(arena, &reg, root, source, source_len, module_qn);

    // 3c. Finalize registry — builds hash buckets for O(1) lookups in
    // cbm_registry_lookup_method / lookup_type. Without this, every
    // lookup falls through to the O(N) linear scan in type_registry.c,
    // turning the resolution pass below into an O(N·C·F) cost per file
    // (kubernetes: ~64B strcmps observed = 35min). Must come AFTER all
    // mutations (Phase 1b/1c above) and BEFORE the LSP context init.
    cbm_registry_finalize(&reg);

    // 4. Build LSP context and run
    GoLSPContext ctx;
    go_lsp_init(&ctx, arena, source, source_len, &reg, module_qn, out);
    ctx.rel_path = rel_path;
    ctx.cancelled = cancelled;

    for (int i = 0; i < import_count; i++) {
        if (import_names[i] && import_qns[i]) {
            go_lsp_add_import(&ctx, import_names[i], import_qns[i]);
        }
    }

    CBMGoResolveStatus status = go_lsp_process_file(&ctx, root);

    // 5. Cleanup — only free if we allocated
    if (owns_tree) {
        ts_tree_delete(tree);
        if (parser)
            ts_parser_delete(parser);
    }
    return status;
}

void cbm_run_go_lsp_cross(
    CBMArena *arena, const char *source, int source_len, const char *module_qn,
    const char *rel_path, CBMLSPDef *defs, int def_count, const char **import_names,
    const char **import_qns, int import_count, TSTree *cached_tree, CBMResolvedCallArray *out) {
    (void)cbm_run_go_lsp_cross_status(arena, source, source_len, module_qn, rel_path, defs, def_count,
                                     import_names, import_qns, import_count, cached_tree, out, NULL);
}

/* ── Tier 2: pre-built per-language registry (gopls package summary pattern) ──
 *
 * cbm_go_build_cross_registry runs the registry build ONCE per project (in
 * pipeline.c, before the parallel resolve workers fire). The returned
 * registry is finalized (O(1) lookups) and shared READ-ONLY across all
 * workers in the resolve phase.
 *
 * cbm_run_go_lsp_cross_with_registry is the per-file entrypoint used by
 * the resolve worker. It SKIPS the registry build (uses the pre-built reg)
 * AND skips Phase 1b/1c AST scans (those mutate the registry, which would
 * race with other workers reading the shared reg). Accuracy trade-off:
 * file-local type aliases and struct embeddings discovered ONLY by Phase
 * 1b (and not already in defs[] embedded_types/field_defs strings) are
 * missed. In practice the per-file LSP during extract already captures
 * those via the def strings. */
CBMTypeRegistry *cbm_go_build_cross_registry(CBMArena *arena, CBMLSPDef *defs, int def_count) {
    if (!arena)
        return NULL;
    CBMTypeRegistry *reg = (CBMTypeRegistry *)cbm_arena_alloc(arena, sizeof(*reg));
    if (!reg)
        return NULL;
    cbm_registry_init(reg, arena);
    cbm_go_stdlib_register(reg, arena);

    CBMArena memo_arena;
    cbm_arena_init_lazy(&memo_arena, GO_MEMO_ARENA_BLOCK);
    GoTypeRows own_types = {.arena = &memo_arena};
    GoLSPContext context = {.arena = arena, .package_qn = ""};
    go_register_definitions((GoAstRegistry){&context, reg, &own_types}, defs, def_count,
                             GO_DEFS_PROJECT);
    cbm_arena_destroy(&memo_arena);

    cbm_registry_finalize(reg);
    reg->read_only = true; /* seal: shared Tier-2 registry is read-only during resolve */
    return reg;
}

CBMGoResolveStatus cbm_run_go_lsp_cross_with_registry_status(
    CBMArena *arena, const char *source, int source_len, const char *module_qn,
    const char *rel_path, CBMTypeRegistry *reg, const char **import_names,
    const char **import_qns, int import_count, TSTree *cached_tree,
    CBMResolvedCallArray *out, const _Atomic int *cancelled) {
    if (go_token_cancelled(cancelled))
        return CBM_GO_RESOLVE_CANCELLED;
    if (!source || source_len <= 0 || !reg)
        return CBM_GO_RESOLVE_UNAVAILABLE;

    TSParser *parser = NULL;
    TSTree *tree = cached_tree;
    bool owns_tree = false;
    if (!tree) {
        parser = ts_parser_new();
        if (!parser)
            return CBM_GO_RESOLVE_FAILED;
        if (!ts_parser_set_language(parser, tree_sitter_go())) {
            ts_parser_delete(parser);
            return CBM_GO_RESOLVE_UNAVAILABLE;
        }
        tree = cbm_parse_source(parser, source, (uint32_t)source_len, (TSParseOptions){0});
        owns_tree = true;
        if (!tree) {
            ts_parser_delete(parser);
            return CBM_GO_RESOLVE_UNAVAILABLE;
        }
    }
    TSNode root = ts_tree_root_node(tree);

    /* Phase 1b/1c (per-file AST mutations on registry) DELIBERATELY SKIPPED —
     * shared registry must stay immutable across parallel workers. */

    GoLSPContext ctx;
    go_lsp_init(&ctx, arena, source, source_len, reg, module_qn, out);
    ctx.rel_path = rel_path;
    ctx.cancelled = cancelled;
    for (int i = 0; i < import_count; i++) {
        if (import_names[i] && import_qns[i]) {
            go_lsp_add_import(&ctx, import_names[i], import_qns[i]);
        }
    }
    CBMGoResolveStatus status = go_lsp_process_file(&ctx, root);

    if (owns_tree) {
        ts_tree_delete(tree);
        if (parser)
            ts_parser_delete(parser);
    }
    return status;
}

void cbm_run_go_lsp_cross_with_registry(
    CBMArena *arena, const char *source, int source_len, const char *module_qn,
    const char *rel_path, CBMTypeRegistry *reg, const char **import_names,
    const char **import_qns, int import_count, TSTree *cached_tree, CBMResolvedCallArray *out) {
    (void)cbm_run_go_lsp_cross_with_registry_status(
        arena, source, source_len, module_qn, rel_path, reg, import_names, import_qns, import_count,
        cached_tree, out, NULL);
}

/* Stage Rust metadata results in a private array view until a whole file's
 * resolution finishes. The result arena owns both the input and output. */
typedef struct {
    CBMFileResult *result;
    const CBMTypeRegistry *registry;
    const _Atomic int *cancelled;
    CBMResolvedCallArray staged;
} GoMetadataContext;

typedef struct {
    const char *qualified_name;
    const char *signature;
} GoCallableIdentity;

typedef struct {
    GoTypeBytes caller;
    GoTypeBytes callee;
    GoTypeBytes strategy;
    GoTypeBytes signature;
    float confidence;
    uint32_t kind;
    uint32_t start;
    uint32_t end;
    uint32_t origin;
} GoMetadataRow;

typedef GoCallableIdentity (*GoMetadataLookup)(void *, GoTypeBytes);
typedef bool (*GoMetadataCancelled)(void *);
typedef bool (*GoMetadataStage)(void *, const GoMetadataRow *);
extern CBMGoResolveStatus mori_go_resolve_metadata(
    const CBMResolvedCall *, size_t, const char *, const char *const *, const char *const *,
    size_t, void *, GoMetadataLookup, GoMetadataCancelled, GoMetadataStage);

static GoCallableIdentity go_metadata_lookup(void *opaque, GoTypeBytes name) {
    const GoMetadataContext *ctx = opaque;
    /* Rust passes a terminated lookup key, including the trailing NUL. */
    CBM_MSAN_WRITTEN(name.data, name.len + 1);
    const CBMRegisteredFunc *fn = cbm_registry_lookup_func(ctx->registry, (const char *)name.data);
    return fn ? (GoCallableIdentity){fn->qualified_name, fn->sig} : (GoCallableIdentity){0};
}

static bool go_metadata_cancelled(void *opaque) {
    const GoMetadataContext *ctx = opaque;
    return go_token_cancelled(ctx->cancelled);
}

static char *go_metadata_copy(CBMArena *arena, GoTypeBytes bytes) {
    CBM_MSAN_WRITTEN(bytes.data, bytes.len);
    return cbm_arena_strndup(arena, (const char *)bytes.data, bytes.len);
}

static bool go_metadata_stage(void *opaque, const GoMetadataRow *row) {
    GoMetadataContext *ctx = opaque;
    CBMArena *arena = &ctx->result->arena;
    CBM_MSAN_WRITTEN(row, sizeof(*row));
    CBMResolvedCall call = {
        .caller_qn = go_metadata_copy(arena, row->caller),
        .callee_qn = go_metadata_copy(arena, row->callee),
        .strategy = go_metadata_copy(arena, row->strategy),
        .confidence = row->confidence,
        .kind = (CBMResolvedKind)row->kind,
        .site_start_byte = row->start,
        .site_end_byte = row->end,
        .source_origin = (CBMSourceOrigin)row->origin,
        .callee_sig = row->signature.data ? go_metadata_copy(arena, row->signature) : NULL,
    };
    if (!call.caller_qn || !call.callee_qn || !call.strategy ||
        (row->signature.data && !call.callee_sig))
        return false;
    int prior_count = ctx->staged.count;
    cbm_resolvedcall_push(&ctx->staged, arena, call);
    return ctx->staged.count > prior_count;
}

CBMGoResolveStatus cbm_go_resolve_cross_metadata(
    CBMFileResult *result, const CBMTypeRegistry *reg, const char **import_names,
    const char **import_qns, int import_count, const _Atomic int *cancelled) {
    if (!result || !reg || import_count < 0 || (import_count && (!import_names || !import_qns)) ||
        result->resolved_calls.count < 0 ||
        (result->resolved_calls.count && !result->resolved_calls.items))
        return CBM_GO_RESOLVE_FAILED;
    GoMetadataContext ctx = {result, reg, cancelled, result->resolved_calls};
    CBMGoResolveStatus status = mori_go_resolve_metadata(
        result->resolved_calls.items, (size_t)result->resolved_calls.count, result->module_qn,
        import_names, import_qns, (size_t)import_count, &ctx, go_metadata_lookup,
        go_metadata_cancelled, go_metadata_stage);
    if (status == CBM_GO_RESOLVE_COMPLETE)
        result->resolved_calls = ctx.staged;
    return status;
}
