#include "go_lsp.h"
#include "../native_tree_ffi.h"
#include "lsp_node_iter.h"
#include "lsp_neg_memo.h"
#include "lsp_work.h"
#include "../../../src/foundation/hash_table.h"
#include "../helpers.h"
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>

static void extract_type_params_from_ast(CBMArena *arena, CBMTypeRegistry *reg, TSNode root,
                                         const char *source, int source_len, const char *module_qn);

// --- Initialization ---

void go_lsp_init(GoLSPContext *ctx, CBMArena *arena, const char *source, int source_len,
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

void go_lsp_add_import(GoLSPContext *ctx, const char *local_name, const char *pkg_qn) {
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

const CBMType *go_parse_type_node(GoLSPContext *ctx, TSNode node) {
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
    CBMScope *(*push_scope)(GoLSPContext *);
    void (*pop_scope)(GoLSPContext *, CBMScope *);
    void (*bind)(GoLSPContext *, const char *, const CBMType *);
    void (*bind_value)(GoLSPContext *, const char *, const CBMType *, const char *);
    void (*enter_function)(GoLSPContext *, const TSNode *, const char *);
    const char *(*scope_callable)(GoLSPContext *, const char *);
    void (*clear_callable)(GoLSPContext *, const char *);
    void (*emit)(GoLSPContext *, const GoEvalOutput *);
} GoEvalOps;

typedef struct {
    TSNode root;
    const unsigned char *source;
    size_t source_len;
    const char *package;
    CBMArena *arena;
    const CBMTypeRegistry *registry;
    GoLSPContext *context;
    const GoEvalOps *operations;
    size_t depth_limit;
    const char *const *import_names;
    const char *const *import_values;
    size_t import_count;
} GoEvalInput;

extern void mori_go_walk_file(const GoEvalInput *input);

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

static CBMScope *go_eval_push_scope(GoLSPContext *ctx) {
    CBMScope *parent = ctx->current_scope;
    ctx->current_scope = cbm_scope_push(ctx->arena, parent);
    return parent;
}

static void go_eval_pop_scope(GoLSPContext *ctx, CBMScope *parent) {
    ctx->current_scope = parent;
}

static void go_eval_bind(GoLSPContext *ctx, const char *name, const CBMType *type) {
    cbm_scope_bind(ctx->current_scope, name, type);
}

static void go_eval_bind_value(GoLSPContext *ctx, const char *name, const CBMType *type,
                               const char *callable) {
    cbm_scope_bind_callable(ctx->current_scope, name, type, callable);
}

static const char *go_eval_scope_callable(GoLSPContext *ctx, const char *name) {
    return cbm_scope_lookup_callable(ctx->current_scope, name);
}

static void go_eval_clear_callable(GoLSPContext *ctx, const char *name) {
    cbm_scope_update_callable(ctx->current_scope, name, NULL);
}

static void go_eval_emit(GoLSPContext *ctx, const GoEvalOutput *row) {
    if (!ctx->resolved_calls || !ctx->enclosing_func_qn)
        return;
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
    cbm_resolvedcall_push(ctx->resolved_calls, ctx->arena, call);
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

/* Reuse the same init and method identity helpers as definition extraction. */
static void go_eval_enter_function(GoLSPContext *ctx, const TSNode *node, const char *name) {
    cbm_ffi_ts_node_written(node);
    ctx->enclosing_func_qn = cbm_arena_sprintf(ctx->arena, "%s.%s", ctx->package_qn, name);
    if (strcmp(ts_node_type(*node), "method_declaration") == 0) {
        TSNode recv = ts_node_child_by_field_name(*node, "receiver", 8);
        if (!ts_node_is_null(recv)) {
            char *type = cbm_go_receiver_type_name(ctx->arena, recv, ctx->source);
            if (type && type[0])
                ctx->enclosing_func_qn = cbm_arena_sprintf(ctx->arena, "%s.%s.%s", ctx->package_qn,
                                                           type, name);
        }
    } else if (strcmp(name, "init") == 0) {
        ctx->enclosing_func_qn = cbm_go_init_qn(ctx->arena, ctx->enclosing_func_qn, ctx->rel_path,
                                              *node, ctx->source);
    }
}

void go_lsp_process_file(GoLSPContext *ctx, TSNode node) {
    if (ts_node_is_null(node) || !ctx->source || ctx->source_len < 0 || !ctx->package_qn)
        return;
    static const GoEvalOps ops = {
        .scope_lookup = go_eval_scope_lookup,
        .import = resolve_import,
        .parse_type = go_eval_parse_type,
        .push_scope = go_eval_push_scope,
        .pop_scope = go_eval_pop_scope,
        .bind = go_eval_bind,
        .bind_value = go_eval_bind_value,
        .enter_function = go_eval_enter_function,
        .scope_callable = go_eval_scope_callable,
        .clear_callable = go_eval_clear_callable,
        .emit = go_eval_emit,
    };
    GoEvalInput input = {
        .root = node,
        .source = (const unsigned char *)ctx->source,
        .source_len = (size_t)ctx->source_len,
        .package = ctx->package_qn,
        .arena = ctx->arena,
        .registry = ctx->registry,
        .context = ctx,
        .operations = &ops,
        .depth_limit = (size_t)cbm_lsp_max_walk_depth(),
        .import_names = ctx->import_local_names,
        .import_values = ctx->import_package_qns,
        .import_count = (size_t)ctx->import_count,
    };
    mori_go_walk_file(&input);
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

// --- Helper: parse Go return type text into CBMType ---

const CBMType *cbm_parse_return_type_text(CBMArena *a, const char *text, const char *module_qn) {
    if (!text || !text[0])
        return cbm_type_unknown();

    // Pointer: *Foo
    if (text[0] == '*') {
        return cbm_type_pointer(a, cbm_parse_return_type_text(a, text + 1, module_qn));
    }

    // Slice: []Foo
    if (text[0] == '[' && text[1] == ']') {
        return cbm_type_slice(a, cbm_parse_return_type_text(a, text + 2, module_qn));
    }

    // Builtin types
    static const char *builtins[] = {"int",     "int8",    "int16",  "int32",   "int64",
                                     "uint",    "uint8",   "uint16", "uint32",  "uint64",
                                     "float32", "float64", "string", "bool",    "byte",
                                     "rune",    "error",   "any",    "uintptr", NULL};
    for (const char **b = builtins; *b; b++) {
        if (strcmp(text, *b) == 0)
            return cbm_type_builtin(a, text);
    }

    // Named type: assume local to module
    return cbm_type_named(a, cbm_arena_sprintf(a, "%s.%s", module_qn, text));
}

typedef struct {
    const char *module_qn;
} CBMGoLSPSignatureParserContext;

static const CBMType *cbm_go_lsp_parse_signature_param_text(CBMArena *arena, const char *text,
                                                            void *parser_ctx) {
    const CBMGoLSPSignatureParserContext *ctx = (const CBMGoLSPSignatureParserContext *)parser_ctx;
    const char *module_qn = ctx && ctx->module_qn ? ctx->module_qn : "";
    return cbm_parse_return_type_text(arena, text, module_qn);
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

// --- Entry point: build registry from file defs + run LSP ---

void cbm_run_go_lsp(CBMArena *arena, CBMFileResult *result, const char *source, int source_len,
                    TSNode root, const char *rel_path) {

    CBMTypeRegistry reg;
    cbm_registry_init(&reg, arena);

    // Go stdlib types/functions: the shared sealed registry, chained
    go_registry_with_stdlib(&reg, arena);

    /* Rows of the file's own registry by QN, from whatever it holds already (the
     * stdlib, when it could not be shared) to the types registered below. */
    CBMArena memo_arena;
    cbm_arena_init_lazy(&memo_arena, GO_MEMO_ARENA_BLOCK);
    GoTypeRows own_types = {.arena = &memo_arena};
    for (int ti = 0; ti < reg.type_count; ti++)
        go_type_rows_note(&own_types, &reg, ti);

    const char *module_qn = result->module_qn;

    // Phase 1: Register all types and functions from the file's own definitions
    for (int i = 0; i < result->defs.count; i++) {
        CBMDefinition *d = &result->defs.items[i];
        if (!d->qualified_name || !d->name)
            continue;

        // Register every type-like container (Class/Struct/Type/Interface/Enum/
        // Trait). Struct included so a Go `type T struct {...}` (now labelled
        // "Struct") is registered as a type and its methods/embedding resolve.
        if (cbm_label_is_type_like(d->label)) {
            CBMRegisteredType rt;
            memset(&rt, 0, sizeof(rt));
            rt.qualified_name = d->qualified_name;
            rt.short_name = d->name;
            rt.is_interface = d->label && strcmp(d->label, "Interface") == 0;

            // Populate embedded_types from base_classes (Go struct embedding)
            if (d->base_classes) {
                int bc_count = 0;
                while (d->base_classes[bc_count])
                    bc_count++;
                if (bc_count > 0) {
                    const char **embedded = (const char **)cbm_arena_alloc(
                        arena, (bc_count + 1) * sizeof(const char *));
                    for (int j = 0; j < bc_count; j++) {
                        const char *bc = d->base_classes[j];
                        // Strip pointer prefix for embedded *Type
                        while (bc[0] == '*')
                            bc++;
                        // Qualify the embedded type name
                        embedded[j] = cbm_arena_sprintf(arena, "%s.%s", module_qn, bc);
                    }
                    embedded[bc_count] = NULL;
                    rt.embedded_types = embedded;
                }
            }

            go_type_rows_add(&reg, &own_types, rt);
        }

        // Register Function/Method nodes
        if (d->label && (strcmp(d->label, "Function") == 0 || strcmp(d->label, "Method") == 0)) {
            CBMRegisteredFunc rf;
            memset(&rf, 0, sizeof(rf));
            rf.qualified_name = d->qualified_name;
            rf.short_name = d->name;

            // Build FUNC type from return_types
            const CBMType **ret_types = NULL;
            if (d->return_types) {
                int count = 0;
                while (d->return_types[count])
                    count++;
                if (count > 0) {
                    ret_types = (const CBMType **)cbm_arena_alloc(
                        arena, (count + 1) * sizeof(const CBMType *));
                    for (int j = 0; j < count; j++) {
                        ret_types[j] =
                            cbm_parse_return_type_text(arena, d->return_types[j], module_qn);
                    }
                    ret_types[count] = NULL;
                }
            } else if (d->return_type && d->return_type[0]) {
                // Fallback: single return_type string
                ret_types = (const CBMType **)cbm_arena_alloc(arena, 2 * sizeof(const CBMType *));
                ret_types[0] = cbm_parse_return_type_text(arena, d->return_type, module_qn);
                ret_types[1] = NULL;
            }

            /* The occurrence-preserving carrier is authoritative whenever it
             * is present, including a non-NULL carrier with an explicit zero
             * count. Its slots deliberately have no names because legacy name
             * extraction is not guaranteed to have the same multiplicity. */
            const CBMType **param_types_arr = NULL;
            const char **param_names_arr = NULL;
            bool has_ordered_params =
                d->signature_param_types != NULL || d->signature_param_count > 0;
            CBMGoLSPSignatureParserContext param_parser_ctx = {module_qn};
            if (has_ordered_params) {
                param_types_arr = cbm_type_materialize_signature_params(
                    arena, d->signature_param_types, d->signature_param_count,
                    cbm_go_lsp_parse_signature_param_text, &param_parser_ctx);
            } else if (d->param_types) {
                param_names_arr = d->param_names;
                int count = 0;
                while (d->param_types[count])
                    count++;
                if (count > 0) {
                    param_types_arr = (const CBMType **)cbm_arena_alloc(
                        arena, (count + 1) * sizeof(const CBMType *));
                    for (int j = 0; j < count; j++) {
                        param_types_arr[j] =
                            cbm_parse_return_type_text(arena, d->param_types[j], module_qn);
                    }
                    param_types_arr[count] = NULL;
                }
            } else {
                param_names_arr = d->param_names;
            }

            rf.signature = cbm_type_func(arena, param_names_arr, param_types_arr, ret_types);

            // Prefer the extractor's semantic receiver QN. Raw receiver text is
            // retained only as a compatibility fallback for hand-built defs;
            // reparsing it alone loses valid unnamed receivers such as `(Right)`.
            if (strcmp(d->label, "Method") == 0) {
                if (d->parent_class && d->parent_class[0]) {
                    rf.receiver_type = d->parent_class;
                } else if (d->receiver && d->receiver[0]) {
                    // Legacy named receiver formats: "(name *Type)" / "(name Type)".
                    const char *r = d->receiver;
                    while (*r == '(' || *r == ' ')
                        r++;
                    while (*r && *r != ' ' && *r != '*')
                        r++;
                    while (*r == ' ' || *r == '*')
                        r++;
                    const char *end = r;
                    while (*end && *end != ')' && *end != ' ')
                        end++;
                    if (end > r) {
                        char *type_name = cbm_arena_strndup(arena, r, (size_t)(end - r));
                        rf.receiver_type = cbm_arena_sprintf(arena, "%s.%s", module_qn, type_name);
                    }
                }

                /* reg's own types are all in own_types; the stdlib is behind. */
                if (rf.receiver_type &&
                    go_type_rows_first(&own_types, &reg, rf.receiver_type) < 0 &&
                    !cbm_registry_lookup_type(reg.fallback, rf.receiver_type)) {
                    const char *dot = strrchr(rf.receiver_type, '.');
                    CBMRegisteredType auto_type;
                    memset(&auto_type, 0, sizeof(auto_type));
                    auto_type.qualified_name = rf.receiver_type;
                    auto_type.short_name = dot ? dot + 1 : rf.receiver_type;
                    go_type_rows_add(&reg, &own_types, auto_type);
                }
            }

            cbm_registry_add_func(&reg, rf);
        }
    }

    // Phase 1b: Scan AST for struct definitions to populate embedded_types
    {
        // Collect top-level children once (O(n)); ts_node_child(root,i) is O(i) → O(n²).
        uint32_t rkn = 0;
        TSNode *rkids = cbm_lsp_collect_children(arena, root, &rkn);
        for (uint32_t i = 0; i < rkn; i++) {
            TSNode top = rkids[i];
            const char *tk = ts_node_type(top);
            // type_declaration contains type_spec children
            if (strcmp(tk, "type_declaration") != 0)
                continue;
            uint32_t td_nc = ts_node_child_count(top);
            for (uint32_t j = 0; j < td_nc; j++) {
                TSNode spec = ts_node_child(top, j);
                if (ts_node_is_null(spec) || !ts_node_is_named(spec))
                    continue;
                const char *spec_kind = ts_node_type(spec);

                // type_alias: type Foo = Bar
                if (strcmp(spec_kind, "type_alias") == 0) {
                    TSNode alias_name = ts_node_child_by_field_name(spec, "name", 4);
                    TSNode alias_type = ts_node_child_by_field_name(spec, "type", 4);
                    if (!ts_node_is_null(alias_name) && !ts_node_is_null(alias_type)) {
                        char *aname = cbm_node_text(arena, alias_name, source);
                        char *atarget = cbm_node_text(arena, alias_type, source);
                        if (aname && aname[0] && atarget && atarget[0]) {
                            const char *alias_type_qn =
                                cbm_arena_sprintf(arena, "%s.%s", module_qn, aname);
                            const char *alias_target_qn =
                                cbm_arena_sprintf(arena, "%s.%s", module_qn, atarget);
                            int alias_at = go_type_rows_first(&own_types, &reg, alias_type_qn);
                            if (alias_at >= 0) {
                                reg.types[alias_at].alias_of = alias_target_qn;
                            } else {
                                CBMRegisteredType alias_rt;
                                memset(&alias_rt, 0, sizeof(alias_rt));
                                alias_rt.qualified_name = alias_type_qn;
                                alias_rt.short_name = aname;
                                alias_rt.alias_of = alias_target_qn;
                                go_type_rows_add(&reg, &own_types, alias_rt);
                            }
                        }
                    }
                    continue;
                }

                if (strcmp(spec_kind, "type_spec") != 0)
                    continue;

                TSNode name_node = ts_node_child_by_field_name(spec, "name", 4);
                TSNode type_node = ts_node_child_by_field_name(spec, "type", 4);
                if (ts_node_is_null(name_node) || ts_node_is_null(type_node))
                    continue;

                char *type_name = cbm_node_text(arena, name_node, source);
                if (!type_name || !type_name[0])
                    continue;
                const char *type_qn = cbm_arena_sprintf(arena, "%s.%s", module_qn, type_name);

                // Interface type: extract method names for satisfaction checking
                if (strcmp(ts_node_type(type_node), "interface_type") == 0) {
                    const char *iface_methods[64];
                    int iface_method_count = 0;
                    uint32_t inl_nc = ts_node_named_child_count(type_node);
                    for (uint32_t k = 0; k < inl_nc && iface_method_count < 63; k++) {
                        TSNode child = ts_node_named_child(type_node, k);
                        if (ts_node_is_null(child))
                            continue;
                        const char *ck = ts_node_type(child);
                        // method_spec: MethodName(params) returns
                        if (strcmp(ck, "method_spec") == 0 || strcmp(ck, "method_elem") == 0) {
                            TSNode mname = ts_node_child_by_field_name(child, "name", 4);
                            if (!ts_node_is_null(mname)) {
                                char *mn = cbm_node_text(arena, mname, source);
                                if (mn && mn[0]) {
                                    iface_methods[iface_method_count++] = mn;
                                }
                            }
                        }
                    }
                    int iface_at =
                        iface_method_count > 0 ? go_type_rows_first(&own_types, &reg, type_qn) : -1;
                    if (iface_at >= 0) {
                        const char **names = (const char **)cbm_arena_alloc(
                            arena, (iface_method_count + 1) * sizeof(const char *));
                        for (int mi = 0; mi < iface_method_count; mi++) {
                            names[mi] = iface_methods[mi];
                        }
                        names[iface_method_count] = NULL;
                        reg.types[iface_at].method_names = names;
                    }
                    continue;
                }

                if (strcmp(ts_node_type(type_node), "struct_type") != 0)
                    continue;

                // Find field_declaration_list inside struct_type
                TSNode field_list = ts_node_child_by_field_name(type_node, "body", 4);
                if (ts_node_is_null(field_list)) {
                    // Some grammars use first named child
                    if (ts_node_named_child_count(type_node) > 0)
                        field_list = ts_node_named_child(type_node, 0);
                }
                if (ts_node_is_null(field_list))
                    continue;

                // Scan field_declarations for embeds and named fields
                const char *embeds[16];
                int embed_count = 0;
                const char *fld_names[64];
                const CBMType *fld_types[64];
                int fld_count = 0;

                // Create a temporary LSP context for parsing field types
                GoLSPContext tmp_ctx;
                memset(&tmp_ctx, 0, sizeof(tmp_ctx));
                tmp_ctx.arena = arena;
                tmp_ctx.source = source;
                tmp_ctx.source_len = source_len;
                tmp_ctx.registry = &reg;
                tmp_ctx.package_qn = module_qn;

                uint32_t fl_nc = ts_node_child_count(field_list);
                for (uint32_t k = 0; k < fl_nc; k++) {
                    TSNode field = ts_node_child(field_list, k);
                    if (ts_node_is_null(field) || !ts_node_is_named(field))
                        continue;
                    if (strcmp(ts_node_type(field), "field_declaration") != 0)
                        continue;

                    TSNode fname = ts_node_child_by_field_name(field, "name", 4);
                    TSNode ftype = ts_node_child_by_field_name(field, "type", 4);

                    if (ts_node_is_null(fname) && !ts_node_is_null(ftype)) {
                        // Embedded field: has type but no name
                        if (embed_count < 15) {
                            char *embed_text = cbm_node_text(arena, ftype, source);
                            if (embed_text && embed_text[0]) {
                                const char *et = embed_text;
                                while (*et == '*')
                                    et++;
                                embeds[embed_count++] =
                                    cbm_arena_sprintf(arena, "%s.%s", module_qn, et);
                            }
                        }
                    } else if (!ts_node_is_null(fname) && !ts_node_is_null(ftype) &&
                               fld_count < 63) {
                        // Named field: name + type
                        char *fn = cbm_node_text(arena, fname, source);
                        if (fn && fn[0]) {
                            fld_names[fld_count] = fn;
                            fld_types[fld_count] = go_parse_type_node(&tmp_ctx, ftype);
                            fld_count++;
                        }
                    }
                }

                // Find the registered type and update it
                int ti = go_type_rows_first(&own_types, &reg, type_qn);
                if (ti >= 0) {
                    if (embed_count > 0) {
                        const char **arr = (const char **)cbm_arena_alloc(
                            arena, (embed_count + 1) * sizeof(const char *));
                        for (int ei = 0; ei < embed_count; ei++)
                            arr[ei] = embeds[ei];
                        arr[embed_count] = NULL;
                        reg.types[ti].embedded_types = arr;
                    }
                    if (fld_count > 0) {
                        const char **names = (const char **)cbm_arena_alloc(
                            arena, (fld_count + 1) * sizeof(const char *));
                        const CBMType **types = (const CBMType **)cbm_arena_alloc(
                            arena, (fld_count + 1) * sizeof(const CBMType *));
                        for (int fi = 0; fi < fld_count; fi++) {
                            names[fi] = fld_names[fi];
                            types[fi] = fld_types[fi];
                        }
                        names[fld_count] = NULL;
                        types[fld_count] = NULL;
                        reg.types[ti].field_names = names;
                        reg.types[ti].field_types = types;
                    }
                }
            }
        }
    }
    cbm_arena_destroy(&memo_arena);

    // Phase 1c: Extract type parameters from generic function declarations
    extract_type_params_from_ast(arena, &reg, root, source, source_len, module_qn);

    // Phase 2: Build LSP context and run
    GoLSPContext lsp_ctx;
    go_lsp_init(&lsp_ctx, arena, source, source_len, &reg, module_qn, &result->resolved_calls);
    lsp_ctx.rel_path = rel_path;

    // Add imports
    for (int i = 0; i < result->imports.count; i++) {
        CBMImport *imp = &result->imports.items[i];
        if (imp->local_name && imp->module_path) {
            go_lsp_add_import(&lsp_ctx, imp->local_name, imp->module_path);
        }
    }

    // Process the file
    go_lsp_process_file(&lsp_ctx, root);
}

// --- Cross-file LSP: parse source, build registry from defs, run LSP ---

// Helper: split "|"-separated string into array of CBMType*.
static const CBMType **split_pipe_types(CBMArena *a, const char *text, const char *module_qn) {
    if (!text || !text[0])
        return NULL;

    // Count separators
    int count = 1;
    for (const char *p = text; *p; p++) {
        if (*p == '|')
            count++;
    }

    const CBMType **arr =
        (const CBMType **)cbm_arena_alloc(a, (count + 1) * sizeof(const CBMType *));
    if (!arr)
        return NULL;

    // Split and parse each type
    char *buf = cbm_arena_strdup(a, text);
    int idx = 0;
    char *start = buf;
    for (char *p = buf;; p++) {
        if (*p == '|' || *p == '\0') {
            char save = *p;
            *p = '\0';
            if (start[0]) {
                arr[idx++] = cbm_parse_return_type_text(a, start, module_qn);
            }
            if (save == '\0')
                break;
            start = p + 1;
        }
    }
    arr[idx] = NULL;
    return idx > 0 ? arr : NULL;
}

// Helper: split "|"-separated string into array of const char*.
static const char **split_pipe_strings(CBMArena *a, const char *text) {
    if (!text || !text[0])
        return NULL;

    int count = 1;
    for (const char *p = text; *p; p++) {
        if (*p == '|')
            count++;
    }

    const char **arr = (const char **)cbm_arena_alloc(a, (count + 1) * sizeof(const char *));
    if (!arr)
        return NULL;

    char *buf = cbm_arena_strdup(a, text);
    int idx = 0;
    char *start = buf;
    for (char *p = buf;; p++) {
        if (*p == '|' || *p == '\0') {
            char save = *p;
            *p = '\0';
            if (start[0]) {
                arr[idx++] = cbm_arena_strdup(a, start);
            }
            if (save == '\0')
                break;
            start = p + 1;
        }
    }
    arr[idx] = NULL;
    return idx > 0 ? arr : NULL;
}

/* QN of the type an identity alias `type A = B` names, for the def-driven
 * cross-file registries (#2033). The per-file AST scan sets alias_of only for
 * the file it walks, so an alias declared in another file of the package
 * reaches a call only through here. Only a bare identifier is qualified (B
 * declared in A's package); a qualified, pointer, composite or generic target
 * stays NULL rather than guessed. */
static const char *go_alias_target_qn(CBMArena *arena, const char *target,
                                      const char *def_module_qn) {
    if (!target || !target[0] || !def_module_qn || !def_module_qn[0]) {
        return NULL;
    }
    for (const unsigned char *p = (const unsigned char *)target; *p; p++) {
        bool ident = (*p >= 'a' && *p <= 'z') || (*p >= 'A' && *p <= 'Z') ||
                     (*p >= '0' && *p <= '9') || *p == '_' || *p >= 0x80;
        if (!ident) {
            return NULL;
        }
    }
    return cbm_arena_sprintf(arena, "%s.%s", def_module_qn, target);
}

// Helper: parse "|"-separated "name:type" field definitions and populate a registered type.
// Format: "Binder:Binder|Name:string|Count:int"
// type_text is resolved relative to def_module_qn.
/* Parse "name:type|name:type" into NULL-terminated name/type arrays. False when
 * nothing parsed (or on allocation failure): the type keeps no field info. */
static bool parse_field_defs(CBMArena *arena, const char *field_defs, const char *def_module_qn,
                             const char ***names_out, const CBMType ***types_out) {

    // Count fields
    int count = 1;
    for (const char *p = field_defs; *p; p++) {
        if (*p == '|')
            count++;
    }
    if (count > 63)
        count = 63;

    const char **names = (const char **)cbm_arena_alloc(arena, (count + 1) * sizeof(const char *));
    const CBMType **types =
        (const CBMType **)cbm_arena_alloc(arena, (count + 1) * sizeof(const CBMType *));
    if (!names || !types)
        return false;

    char *buf = cbm_arena_strdup(arena, field_defs);
    int idx = 0;
    char *start = buf;
    for (char *p = buf;; p++) {
        if (*p == '|' || *p == '\0') {
            char save = *p;
            *p = '\0';
            // Parse "name:type"
            char *colon = NULL;
            for (char *q = start; *q; q++) {
                if (*q == ':') {
                    colon = q;
                    break;
                }
            }
            if (colon && idx < count) {
                *colon = '\0';
                names[idx] = cbm_arena_strdup(arena, start);
                types[idx] = cbm_parse_return_type_text(arena, colon + 1, def_module_qn);
                idx++;
            }
            if (save == '\0')
                break;
            start = p + 1;
        }
    }
    names[idx] = NULL;
    types[idx] = NULL;
    *names_out = names;
    *types_out = types;
    return idx > 0;
}

static void parse_field_defs_into_type(CBMArena *arena, CBMTypeRegistry *reg, const char *type_qn,
                                       const char *field_defs, const char *def_module_qn) {
    if (!field_defs || !field_defs[0] || !type_qn)
        return;
    const char **names = NULL;
    const CBMType **types = NULL;
    if (parse_field_defs(arena, field_defs, def_module_qn, &names, &types)) {
        // Find the registered type and update field info
        for (int ti = 0; ti < reg->type_count; ti++) {
            if (reg->types[ti].qualified_name &&
                strcmp(reg->types[ti].qualified_name, type_qn) == 0) {
                reg->types[ti].field_names = names;
                reg->types[ti].field_types = types;
                break;
            }
        }
    }
}

// --- Phase 1c: Extract type parameters from generic function declarations ---

static const CBMType *parse_type_node_with_params(CBMArena *arena, TSNode node, const char *source,
                                                 int source_len, const char *module_qn,
                                                 const char **type_param_names) {
    if (ts_node_is_null(node) || !source || source_len < 0 || !module_qn)
        return cbm_type_unknown();
    GoLSPContext ctx = {.arena = arena};
    GoTypeBytes bytes = {(const unsigned char *)source, (size_t)source_len};
    return mori_go_parse_type(node, bytes, module_qn, &ctx, NULL, type_param_names, go_type_make);
}

/* Go permits one AST parameter declaration to introduce several call
 * positions: `first, second T`. Count direct names without exposing or
 * rewriting the legacy public param_names projection. */
static int cbm_go_lsp_signature_param_multiplicity(TSNode param) {
    const char *kind = ts_node_type(param);
    if (strcmp(kind, "parameter_declaration") != 0 &&
        strcmp(kind, "variadic_parameter_declaration") != 0) {
        return 1;
    }

    int field_names = 0;
    int identifier_names = 0;
    uint32_t child_count = ts_node_child_count(param);
    for (uint32_t i = 0; i < child_count; i++) {
        TSNode child = ts_node_child(param, i);
        if (ts_node_is_null(child) || !ts_node_is_named(child))
            continue;
        const char *field = ts_node_field_name_for_child(param, i);
        if (field && strcmp(field, "name") == 0)
            field_names++;
        if (strcmp(ts_node_type(child), "identifier") == 0)
            identifier_names++;
    }

    int names = field_names > 0 ? field_names : identifier_names;
    return names > 0 ? names : 1;
}

static const CBMType *cbm_go_lsp_keep_known_refinement(const CBMType *candidate,
                                                       const CBMType *existing) {
    if (cbm_type_is_unknown(candidate) && !cbm_type_is_unknown(existing)) {
        return existing;
    }
    return candidate ? candidate : cbm_type_unknown();
}

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

// Scan AST for generic function declarations and set type_param_names + re-parse
// return types on matching registered functions.
static void extract_type_params_from_ast(CBMArena *arena, CBMTypeRegistry *reg, TSNode root,
                                         const char *source, int source_len, const char *module_qn) {
    GoFuncRows func_rows;
    memset(&func_rows, 0, sizeof(func_rows));
    cbm_arena_init_lazy(&func_rows.arena, GO_MEMO_ARENA_BLOCK);

    // Collect top-level children once (O(n)); ts_node_child(root,i) is O(i) → O(n²).
    uint32_t rkn = 0;
    TSNode *rkids = cbm_lsp_collect_children(arena, root, &rkn);
    for (uint32_t i = 0; i < rkn; i++) {
        TSNode top = rkids[i];
        if (strcmp(ts_node_type(top), "function_declaration") != 0)
            continue;

        // Check for type_parameter_list (field name: "type_parameters", 15 chars)
        TSNode tp_list = ts_node_child_by_field_name(top, "type_parameters", 15);
        if (ts_node_is_null(tp_list))
            continue;

        // Get function name
        TSNode name_node = ts_node_child_by_field_name(top, "name", 4);
        if (ts_node_is_null(name_node))
            continue;
        char *func_name = cbm_node_text(arena, name_node, source);
        if (!func_name || !func_name[0])
            continue;

        // Extract type param names from type_parameter_declaration children
        const char *params[16];
        int param_count = 0;
        uint32_t tp_nc = ts_node_child_count(tp_list);
        for (uint32_t j = 0; j < tp_nc && param_count < 15; j++) {
            TSNode child = ts_node_child(tp_list, j);
            if (ts_node_is_null(child) || !ts_node_is_named(child))
                continue;
            if (strcmp(ts_node_type(child), "type_parameter_declaration") != 0)
                continue;
            TSNode pname = ts_node_child_by_field_name(child, "name", 4);
            if (ts_node_is_null(pname)) {
                // Fallback: first type_identifier child
                uint32_t cc = ts_node_child_count(child);
                for (uint32_t k = 0; k < cc; k++) {
                    TSNode c = ts_node_child(child, k);
                    if (!ts_node_is_null(c) && ts_node_is_named(c) &&
                        strcmp(ts_node_type(c), "type_identifier") == 0) {
                        pname = c;
                        break;
                    }
                }
            }
            if (!ts_node_is_null(pname)) {
                char *pn = cbm_node_text(arena, pname, source);
                if (pn && pn[0]) {
                    params[param_count++] = cbm_arena_strdup(arena, pn);
                }
            }
        }
        if (param_count == 0)
            continue;
        params[param_count] = NULL;

        // Build arena-allocated type_param_names array
        const char **tp_names =
            (const char **)cbm_arena_alloc(arena, (param_count + 1) * sizeof(const char *));
        for (int j = 0; j <= param_count; j++)
            tp_names[j] = params[j];

        // Find the matching registered function and set type_param_names
        const char *func_qn = cbm_arena_sprintf(arena, "%s.%s", module_qn, func_name);
        int first_fi = go_func_rows_first(&func_rows, reg, func_qn);
        for (int fi = first_fi >= 0 ? first_fi : reg->func_count; fi < reg->func_count; fi++) {
            if (reg->funcs[fi].qualified_name &&
                strcmp(reg->funcs[fi].qualified_name, func_qn) == 0) {
                reg->funcs[fi].type_param_names = tp_names;

                // Re-parse return types with type param awareness
                if (reg->funcs[fi].signature && reg->funcs[fi].signature->kind == CBM_TYPE_FUNC &&
                    reg->funcs[fi].signature->data.func.return_types) {
                    const CBMType **old_rets = reg->funcs[fi].signature->data.func.return_types;
                    int ret_count = 0;
                    while (old_rets[ret_count])
                        ret_count++;

                    // Check if any return type is a NAMED that matches a type param
                    bool needs_reparse = false;
                    for (int ri = 0; ri < ret_count && !needs_reparse; ri++) {
                        const CBMType *check = old_rets[ri];
                        // Unwrap slice/pointer to get inner named type
                        if (check->kind == CBM_TYPE_SLICE && check->data.slice.elem)
                            check = check->data.slice.elem;
                        else if (check->kind == CBM_TYPE_POINTER && check->data.pointer.elem)
                            check = check->data.pointer.elem;
                        if (check->kind == CBM_TYPE_NAMED) {
                            const char *qn = check->data.named.qualified_name;
                            const char *dot = strrchr(qn, '.');
                            const char *short_name = dot ? dot + 1 : qn;
                            for (int pi = 0; pi < param_count; pi++) {
                                if (strcmp(short_name, tp_names[pi]) == 0) {
                                    needs_reparse = true;
                                    break;
                                }
                            }
                        }
                    }

                    if (needs_reparse) {
                        TSNode result_node = ts_node_child_by_field_name(top, "result", 6);
                        if (!ts_node_is_null(result_node)) {
                            const CBMType **new_rets = (const CBMType **)cbm_arena_alloc(
                                arena, (ret_count + 1) * sizeof(const CBMType *));

                            if (strcmp(ts_node_type(result_node), "parameter_list") == 0) {
                                int idx = 0;
                                uint32_t rnc = ts_node_child_count(result_node);
                                for (uint32_t ri = 0; ri < rnc && idx < ret_count; ri++) {
                                    TSNode rchild = ts_node_child(result_node, ri);
                                    if (ts_node_is_null(rchild) || !ts_node_is_named(rchild))
                                        continue;
                                    TSNode rtype = ts_node_child_by_field_name(rchild, "type", 4);
                                    if (ts_node_is_null(rtype))
                                        rtype = rchild;
                                    const CBMType *parsed = parse_type_node_with_params(
                                        arena, rtype, source, source_len, module_qn, tp_names);
                                    new_rets[idx] =
                                        cbm_go_lsp_keep_known_refinement(parsed, old_rets[idx]);
                                    idx++;
                                }
                                while (idx < ret_count) {
                                    new_rets[idx] = old_rets[idx];
                                    idx++;
                                }
                                new_rets[ret_count] = NULL;
                            } else {
                                const CBMType *parsed = parse_type_node_with_params(
                                    arena, result_node, source, source_len, module_qn, tp_names);
                                new_rets[0] = cbm_go_lsp_keep_known_refinement(parsed, old_rets[0]);
                                for (int idx = 1; idx < ret_count; idx++) {
                                    new_rets[idx] = old_rets[idx];
                                }
                                new_rets[ret_count] = NULL;
                            }

                            const CBMType *new_sig = cbm_type_func_replace_returns(
                                arena, reg->funcs[fi].signature, new_rets);
                            if (new_sig && new_sig->kind == CBM_TYPE_FUNC) {
                                reg->funcs[fi].signature = new_sig;
                            }
                        }
                    }
                }

                // Also re-parse param types with TYPE_PARAM awareness (for implicit generics)
                if (reg->funcs[fi].signature && reg->funcs[fi].signature->kind == CBM_TYPE_FUNC &&
                    reg->funcs[fi].signature->data.func.param_types) {
                    const CBMType **old_params = reg->funcs[fi].signature->data.func.param_types;
                    int pc = 0;
                    while (old_params[pc])
                        pc++;
                    // Re-parse from AST parameter list
                    TSNode params_node = ts_node_child_by_field_name(top, "parameters", 10);
                    if (!ts_node_is_null(params_node)) {
                        const CBMType **new_params = (const CBMType **)cbm_arena_alloc(
                            arena, (pc + 1) * sizeof(const CBMType *));
                        int idx = 0;
                        bool overflow = false;
                        uint32_t pnc = ts_node_child_count(params_node);
                        for (uint32_t pi = 0; pi < pnc && !overflow; pi++) {
                            TSNode pchild = ts_node_child(params_node, pi);
                            if (ts_node_is_null(pchild) || !ts_node_is_named(pchild))
                                continue;
                            const char *param_kind = ts_node_type(pchild);
                            if (strcmp(param_kind, "parameter_declaration") != 0 &&
                                strcmp(param_kind, "variadic_parameter_declaration") != 0) {
                                continue;
                            }
                            TSNode ptype = ts_node_child_by_field_name(pchild, "type", 4);
                            const CBMType *parsed =
                                ts_node_is_null(ptype)
                                    ? cbm_type_unknown()
                                    : parse_type_node_with_params(arena, ptype, source, source_len,
                                                                  module_qn, tp_names);
                            int multiplicity = cbm_go_lsp_signature_param_multiplicity(pchild);
                            for (int occurrence = 0; occurrence < multiplicity; occurrence++) {
                                if (idx >= pc) {
                                    overflow = true;
                                    break;
                                }
                                new_params[idx] =
                                    cbm_go_lsp_keep_known_refinement(parsed, old_params[idx]);
                                idx++;
                            }
                        }
                        new_params[idx] = NULL;
                        if (!overflow && idx == pc) {
                            const CBMType *old_sig = reg->funcs[fi].signature;
                            const CBMType *new_sig =
                                cbm_type_func(arena, old_sig->data.func.param_names, new_params,
                                              old_sig->data.func.return_types);
                            if (new_sig && new_sig->kind == CBM_TYPE_FUNC) {
                                reg->funcs[fi].signature = new_sig;
                            }
                        }
                    }
                }
                break;
            }
        }
    }
    cbm_arena_destroy(&func_rows.arena);
}

// Forward: tree-sitter Go language (from lang_specs.c)
extern const TSLanguage *tree_sitter_go(void);

void cbm_run_go_lsp_cross(CBMArena *arena, const char *source, int source_len,
                          const char *module_qn, const char *rel_path, CBMLSPDef *defs,
                          int def_count, const char **import_names, const char **import_qns,
                          int import_count, TSTree *cached_tree, CBMResolvedCallArray *out) {
    if (!source || source_len <= 0)
        return;

    // 1. Use cached tree if available, otherwise parse fresh
    TSParser *parser = NULL;
    TSTree *tree = cached_tree;
    bool owns_tree = false;
    if (!tree) {
        parser = ts_parser_new();
        if (!parser)
            return;
        ts_parser_set_language(parser, tree_sitter_go());
        tree = cbm_parse_source(parser, source, (uint32_t)source_len, (TSParseOptions){0});
        owns_tree = true;
        if (!tree) {
            ts_parser_delete(parser);
            return;
        }
    }
    TSNode root = ts_tree_root_node(tree);

    // 2. Build registry (the stdlib is the shared sealed registry, chained)
    CBMTypeRegistry reg;
    cbm_registry_init(&reg, arena);
    go_registry_with_stdlib(&reg, arena);

    // Register all defs (file-local + cross-file).
    // Perf: borrow strings from defs[] directly — they live in the
    // caller's arena which outlives this call, so cbm_arena_strdup is
    // wasted work. On kubernetes (~110k defs × 11k files × 5 strdups
    // ~= 6B mallocs) this alone saves ~90s of resolve wall time.
    for (int i = 0; i < def_count; i++) {
        CBMLSPDef *d = &defs[i];
        if (!d->qualified_name || !d->short_name || !d->label)
            continue;

        const char *def_mod = d->def_module_qn ? d->def_module_qn : module_qn;

        // Every type-like container (Type/Class/Struct/Interface/Enum/Trait).
        // Struct included so Go structs (now labelled "Struct") register as types.
        if (cbm_label_is_type_like(d->label)) {
            CBMRegisteredType rt;
            memset(&rt, 0, sizeof(rt));
            rt.qualified_name = d->qualified_name; // borrowed
            rt.short_name = d->short_name;         // borrowed
            rt.is_interface = d->is_interface || strcmp(d->label, "Interface") == 0;
            rt.embedded_types = split_pipe_strings(arena, d->embedded_types);

            // Set method_names for interfaces from "|"-separated string
            if (rt.is_interface && d->method_names_str && d->method_names_str[0]) {
                rt.method_names = split_pipe_strings(arena, d->method_names_str);
            }
            rt.alias_of = go_alias_target_qn(arena, d->alias_target, def_mod);

            cbm_registry_add_type(&reg, rt);

            // Populate struct field types from field_defs
            if (d->field_defs && d->field_defs[0]) {
                parse_field_defs_into_type(arena, &reg, rt.qualified_name, d->field_defs, def_mod);
            }
        }

        // Function/Method
        if (strcmp(d->label, "Function") == 0 || strcmp(d->label, "Method") == 0) {
            CBMRegisteredFunc rf;
            memset(&rf, 0, sizeof(rf));
            rf.qualified_name = d->qualified_name; // borrowed
            rf.short_name = d->short_name;         // borrowed

            // Build FUNC type from return_types text
            const CBMType **ret_types = split_pipe_types(arena, d->return_types, def_mod);
            CBMGoLSPSignatureParserContext param_parser_ctx = {def_mod};
            const CBMType **param_types = cbm_type_materialize_signature_params(
                arena, d->signature_param_types, d->signature_param_count,
                cbm_go_lsp_parse_signature_param_text, &param_parser_ctx);
            rf.signature = cbm_type_func(arena, NULL, param_types, ret_types);

            // Method receiver
            if (strcmp(d->label, "Method") == 0 && d->receiver_type && d->receiver_type[0]) {
                rf.receiver_type = d->receiver_type; // borrowed
                // Auto-create type entry if not exists
                if (!cbm_registry_lookup_type(&reg, rf.receiver_type)) {
                    CBMRegisteredType auto_type;
                    memset(&auto_type, 0, sizeof(auto_type));
                    auto_type.qualified_name = rf.receiver_type;
                    const char *dot = strrchr(d->receiver_type, '.');
                    auto_type.short_name = dot ? dot + 1 : rf.receiver_type; // borrowed substring
                    cbm_registry_add_type(&reg, auto_type);
                }
            }

            cbm_registry_add_func(&reg, rf);
        }
    }

    // 3. Phase 1b: Scan AST for struct definitions to populate embedded_types
    {
        /* One context parses every struct's field types, so its import rows are
         * chained once (resolve_import), not once per struct. */
        GoLSPContext tmp_ctx;
        memset(&tmp_ctx, 0, sizeof(tmp_ctx));
        tmp_ctx.arena = arena;
        tmp_ctx.source = source;
        tmp_ctx.source_len = source_len;
        tmp_ctx.registry = &reg;
        tmp_ctx.package_qn = module_qn;
        tmp_ctx.import_local_names = import_names;
        tmp_ctx.import_package_qns = import_qns;
        tmp_ctx.import_count = import_count;

        // Collect top-level children once (O(n)); ts_node_child(root,i) is O(i) → O(n²).
        uint32_t rkn = 0;
        TSNode *rkids = cbm_lsp_collect_children(arena, root, &rkn);
        for (uint32_t i = 0; i < rkn; i++) {
            TSNode top = rkids[i];
            if (strcmp(ts_node_type(top), "type_declaration") != 0)
                continue;
            uint32_t td_nc = ts_node_child_count(top);
            for (uint32_t j = 0; j < td_nc; j++) {
                TSNode spec = ts_node_child(top, j);
                if (ts_node_is_null(spec) || !ts_node_is_named(spec))
                    continue;
                const char *spec_kind2 = ts_node_type(spec);

                // type_alias: type Foo = Bar
                if (strcmp(spec_kind2, "type_alias") == 0) {
                    TSNode alias_name = ts_node_child_by_field_name(spec, "name", 4);
                    TSNode alias_type = ts_node_child_by_field_name(spec, "type", 4);
                    if (!ts_node_is_null(alias_name) && !ts_node_is_null(alias_type)) {
                        char *aname = cbm_node_text(arena, alias_name, source);
                        char *atarget = cbm_node_text(arena, alias_type, source);
                        if (aname && aname[0] && atarget && atarget[0]) {
                            const char *alias_type_qn =
                                cbm_arena_sprintf(arena, "%s.%s", module_qn, aname);
                            const char *alias_target_qn =
                                cbm_arena_sprintf(arena, "%s.%s", module_qn, atarget);
                            bool found_a = false;
                            for (int ti = 0; ti < reg.type_count; ti++) {
                                if (reg.types[ti].qualified_name &&
                                    strcmp(reg.types[ti].qualified_name, alias_type_qn) == 0) {
                                    reg.types[ti].alias_of = alias_target_qn;
                                    found_a = true;
                                    break;
                                }
                            }
                            if (!found_a) {
                                CBMRegisteredType alias_rt;
                                memset(&alias_rt, 0, sizeof(alias_rt));
                                alias_rt.qualified_name = alias_type_qn;
                                alias_rt.short_name = aname;
                                alias_rt.alias_of = alias_target_qn;
                                cbm_registry_add_type(&reg, alias_rt);
                            }
                        }
                    }
                    continue;
                }

                if (strcmp(spec_kind2, "type_spec") != 0)
                    continue;

                TSNode name_node = ts_node_child_by_field_name(spec, "name", 4);
                TSNode type_node = ts_node_child_by_field_name(spec, "type", 4);
                if (ts_node_is_null(name_node) || ts_node_is_null(type_node))
                    continue;
                if (strcmp(ts_node_type(type_node), "struct_type") != 0)
                    continue;

                char *type_name = cbm_node_text(arena, name_node, source);
                if (!type_name || !type_name[0])
                    continue;
                const char *type_qn = cbm_arena_sprintf(arena, "%s.%s", module_qn, type_name);

                TSNode field_list = ts_node_child_by_field_name(type_node, "body", 4);
                if (ts_node_is_null(field_list)) {
                    if (ts_node_named_child_count(type_node) > 0)
                        field_list = ts_node_named_child(type_node, 0);
                }
                if (ts_node_is_null(field_list))
                    continue;

                const char *embeds[16];
                int embed_count = 0;
                const char *fld_names[64];
                const CBMType *fld_types[64];
                int fld_count = 0;

                uint32_t fl_nc = ts_node_child_count(field_list);
                for (uint32_t k = 0; k < fl_nc; k++) {
                    TSNode field = ts_node_child(field_list, k);
                    if (ts_node_is_null(field) || !ts_node_is_named(field))
                        continue;
                    if (strcmp(ts_node_type(field), "field_declaration") != 0)
                        continue;
                    TSNode fname = ts_node_child_by_field_name(field, "name", 4);
                    TSNode ftype = ts_node_child_by_field_name(field, "type", 4);
                    if (ts_node_is_null(fname) && !ts_node_is_null(ftype)) {
                        if (embed_count < 15) {
                            char *embed_text = cbm_node_text(arena, ftype, source);
                            if (embed_text && embed_text[0]) {
                                const char *et = embed_text;
                                while (*et == '*')
                                    et++;
                                embeds[embed_count++] =
                                    cbm_arena_sprintf(arena, "%s.%s", module_qn, et);
                            }
                        }
                    } else if (!ts_node_is_null(fname) && !ts_node_is_null(ftype) &&
                               fld_count < 63) {
                        char *fn = cbm_node_text(arena, fname, source);
                        if (fn && fn[0]) {
                            fld_names[fld_count] = fn;
                            fld_types[fld_count] = go_parse_type_node(&tmp_ctx, ftype);
                            fld_count++;
                        }
                    }
                }

                for (int ti = 0; ti < reg.type_count; ti++) {
                    if (!reg.types[ti].qualified_name ||
                        strcmp(reg.types[ti].qualified_name, type_qn) != 0)
                        continue;

                    if (embed_count > 0) {
                        const char **arr = (const char **)cbm_arena_alloc(
                            arena, (embed_count + 1) * sizeof(const char *));
                        for (int ei = 0; ei < embed_count; ei++)
                            arr[ei] = embeds[ei];
                        arr[embed_count] = NULL;
                        reg.types[ti].embedded_types = arr;
                    }
                    if (fld_count > 0) {
                        const char **names = (const char **)cbm_arena_alloc(
                            arena, (fld_count + 1) * sizeof(const char *));
                        const CBMType **types = (const CBMType **)cbm_arena_alloc(
                            arena, (fld_count + 1) * sizeof(const CBMType *));
                        for (int fi = 0; fi < fld_count; fi++) {
                            names[fi] = fld_names[fi];
                            types[fi] = fld_types[fi];
                        }
                        names[fld_count] = NULL;
                        types[fld_count] = NULL;
                        reg.types[ti].field_names = names;
                        reg.types[ti].field_types = types;
                    }
                    break;
                }
            }
        }
    }

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

    for (int i = 0; i < import_count; i++) {
        if (import_names[i] && import_qns[i]) {
            go_lsp_add_import(&ctx, import_names[i], import_qns[i]);
        }
    }

    go_lsp_process_file(&ctx, root);

    // 5. Cleanup — only free if we allocated
    if (owns_tree) {
        ts_tree_delete(tree);
        if (parser)
            ts_parser_delete(parser);
    }
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

    /* Qualified name -> FIRST type index while the registry is built: it is
     * not finalized until the end, so its own lookup scans every type, and the
     * receiver check below plus the field-definition lookup made the build
     * O(types x (methods + structs)) -- 4.4 s single-threaded on the Go corpus,
     * the scaling lane's parse_field_defs_into_type x2.9 (waste sanitizer,
     * 2026-09-17). First-match semantics are kept exactly: a receiver's
     * placeholder type added before its struct still receives the fields.
     * NULL on allocation failure: the linear paths below run as before. */
    CBMHashTable *first_type = cbm_ht_create((uint32_t)(def_count / 4 + 1024));
    int noted_types = 0;
#define GO_NOTE_NEW_TYPES()                                                        \
    do {                                                                           \
        for (; first_type && noted_types < reg->type_count; noted_types++) {       \
            const char *tqn = reg->types[noted_types].qualified_name;              \
            if (tqn && !cbm_ht_has(first_type, tqn)) {                             \
                cbm_ht_set(first_type, tqn, (void *)(uintptr_t)(noted_types + 1)); \
            }                                                                      \
        }                                                                          \
    } while (0)
    GO_NOTE_NEW_TYPES();

    for (int i = 0; i < def_count; i++) {
        CBMLSPDef *d = &defs[i];
        if (!d->qualified_name || !d->short_name || !d->label)
            continue;
        /* Filter to Go defs only — the all_defs[] array is mixed-language. */
        if (d->lang != CBM_LANG_GO)
            continue;
        /* In the pre-built path every def carries its own def_module_qn
         * (set by cbm_pxc_collect_all_defs). There is no caller module to
         * fall back to — this registry is project-wide, not per-file. */
        const char *def_mod = d->def_module_qn ? d->def_module_qn : "";

        // Every type-like container (Type/Class/Struct/Interface/Enum/Trait).
        // Struct included so Go structs (now labelled "Struct") register as types.
        if (cbm_label_is_type_like(d->label)) {
            CBMRegisteredType rt;
            memset(&rt, 0, sizeof(rt));
            rt.qualified_name = d->qualified_name; /* borrowed */
            rt.short_name = d->short_name;
            rt.is_interface = d->is_interface || strcmp(d->label, "Interface") == 0;
            rt.embedded_types = split_pipe_strings(arena, d->embedded_types);
            if (rt.is_interface && d->method_names_str && d->method_names_str[0]) {
                rt.method_names = split_pipe_strings(arena, d->method_names_str);
            }
            rt.alias_of = go_alias_target_qn(arena, d->alias_target, def_mod);
            cbm_registry_add_type(reg, rt);
            GO_NOTE_NEW_TYPES();
            if (d->field_defs && d->field_defs[0]) {
                if (first_type) {
                    const char **names = NULL;
                    const CBMType **types = NULL;
                    uintptr_t at = (uintptr_t)cbm_ht_get(first_type, rt.qualified_name);
                    if (parse_field_defs(arena, d->field_defs, def_mod, &names, &types) && at) {
                        reg->types[at - 1].field_names = names;
                        reg->types[at - 1].field_types = types;
                    }
                } else {
                    parse_field_defs_into_type(arena, reg, rt.qualified_name, d->field_defs,
                                               def_mod);
                }
            }
        }

        if (strcmp(d->label, "Function") == 0 || strcmp(d->label, "Method") == 0) {
            CBMRegisteredFunc rf;
            memset(&rf, 0, sizeof(rf));
            rf.qualified_name = d->qualified_name; /* borrowed */
            rf.short_name = d->short_name;
            const CBMType **ret_types = split_pipe_types(arena, d->return_types, def_mod);
            CBMGoLSPSignatureParserContext param_parser_ctx = {def_mod};
            const CBMType **param_types = cbm_type_materialize_signature_params(
                arena, d->signature_param_types, d->signature_param_count,
                cbm_go_lsp_parse_signature_param_text, &param_parser_ctx);
            rf.signature = cbm_type_func(arena, NULL, param_types, ret_types);
            if (strcmp(d->label, "Method") == 0 && d->receiver_type && d->receiver_type[0]) {
                rf.receiver_type = d->receiver_type;
                bool known = first_type ? cbm_ht_has(first_type, rf.receiver_type)
                                        : cbm_registry_lookup_type(reg, rf.receiver_type) != NULL;
                if (!known) {
                    CBMRegisteredType auto_type;
                    memset(&auto_type, 0, sizeof(auto_type));
                    auto_type.qualified_name = rf.receiver_type;
                    const char *dot = strrchr(d->receiver_type, '.');
                    auto_type.short_name = dot ? dot + 1 : rf.receiver_type;
                    cbm_registry_add_type(reg, auto_type);
                    GO_NOTE_NEW_TYPES();
                }
            }
            cbm_registry_add_func(reg, rf);
        }
    }
#undef GO_NOTE_NEW_TYPES
    cbm_ht_free(first_type);

    cbm_registry_finalize(reg);
    reg->read_only = true; /* seal: shared Tier-2 registry is read-only during resolve */
    return reg;
}

void cbm_run_go_lsp_cross_with_registry(CBMArena *arena, const char *source, int source_len,
                                        const char *module_qn, const char *rel_path,
                                        CBMTypeRegistry *reg, const char **import_names,
                                        const char **import_qns, int import_count,
                                        TSTree *cached_tree, CBMResolvedCallArray *out) {
    if (!source || source_len <= 0 || !reg)
        return;

    TSParser *parser = NULL;
    TSTree *tree = cached_tree;
    bool owns_tree = false;
    if (!tree) {
        parser = ts_parser_new();
        if (!parser)
            return;
        ts_parser_set_language(parser, tree_sitter_go());
        tree = cbm_parse_source(parser, source, (uint32_t)source_len, (TSParseOptions){0});
        owns_tree = true;
        if (!tree) {
            ts_parser_delete(parser);
            return;
        }
    }
    TSNode root = ts_tree_root_node(tree);

    /* Phase 1b/1c (per-file AST mutations on registry) DELIBERATELY SKIPPED —
     * shared registry must stay immutable across parallel workers. */

    GoLSPContext ctx;
    go_lsp_init(&ctx, arena, source, source_len, reg, module_qn, out);
    ctx.rel_path = rel_path;
    for (int i = 0; i < import_count; i++) {
        if (import_names[i] && import_qns[i]) {
            go_lsp_add_import(&ctx, import_names[i], import_qns[i]);
        }
    }
    go_lsp_process_file(&ctx, root);

    if (owns_tree) {
        ts_tree_delete(tree);
        if (parser)
            ts_parser_delete(parser);
    }
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
    return ctx->cancelled && atomic_load_explicit(ctx->cancelled, memory_order_relaxed);
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

// --- Batch cross-file LSP ---

void cbm_batch_go_lsp_cross(CBMArena *arena, CBMBatchGoLSPFile *files, int file_count,
                            CBMResolvedCallArray *out) {
    if (!files || file_count <= 0 || !out)
        return;

    for (int f = 0; f < file_count; f++) {
        CBMBatchGoLSPFile *file = &files[f];
        memset(&out[f], 0, sizeof(CBMResolvedCallArray));

        if (!file->source || file->source_len <= 0 || file->def_count <= 0)
            continue;

        // Per-file arena: registry + temp data freed after each file
        CBMArena file_arena;
        cbm_arena_init(&file_arena);

        CBMResolvedCallArray file_out;
        memset(&file_out, 0, sizeof(file_out));

        // Delegate to existing per-file function
        cbm_run_go_lsp_cross(&file_arena, file->source, file->source_len, file->module_qn,
                             file->rel_path, file->defs, file->def_count, file->import_names,
                             file->import_qns, file->import_count, file->cached_tree, &file_out);

        // Copy results to output arena (must outlive per-file arena)
        if (file_out.count > 0) {
            out[f].count = file_out.count;
            out[f].items =
                (CBMResolvedCall *)cbm_arena_alloc(arena, file_out.count * sizeof(CBMResolvedCall));
            for (int j = 0; j < file_out.count; j++) {
                CBMResolvedCall *src = &file_out.items[j];
                CBMResolvedCall *dst = &out[f].items[j];
                memset(dst, 0, sizeof(*dst));
                dst->caller_qn = src->caller_qn ? cbm_arena_strdup(arena, src->caller_qn) : NULL;
                dst->callee_qn = src->callee_qn ? cbm_arena_strdup(arena, src->callee_qn) : NULL;
                dst->strategy = src->strategy ? cbm_arena_strdup(arena, src->strategy) : NULL;
                dst->confidence = src->confidence;
                dst->reason = src->reason ? cbm_arena_strdup(arena, src->reason) : NULL;
                dst->kind = src->kind;
                dst->site_start_byte = src->site_start_byte;
                dst->site_end_byte = src->site_end_byte;
            }
        }

        cbm_arena_destroy(&file_arena);
    }
}
