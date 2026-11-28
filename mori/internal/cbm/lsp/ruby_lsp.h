#ifndef CBM_LSP_RUBY_LSP_H
#define CBM_LSP_RUBY_LSP_H

#include "type_rep.h"
#include "scope.h"
#include "type_registry.h"
#include "../cbm.h"

typedef struct RubyAncestry RubyAncestry;

/* One inferred instance-variable type: (class QN, @name) → type QN.
 * `conflicted` latches when two assignments disagree — lookup then returns
 * nothing (zero-edge guarantee beats a coin-flip type). */
typedef struct {
    const char *class_qn;
    const char *ivar_name; /* including the leading '@' */
    const char *type_qn;
    bool conflicted;
} RubyIvarInfo;

/* RubyLSPContext — per-file state for Ruby type-aware call resolution.
 * Mirrors PerlLSPContext / PHPLSPContext structure.
 *
 * Ruby specifics that shape this context:
 *   - Classes/modules nest lexically (`module A; class B`), and constants
 *     resolve through the lexical nesting outward (A::B sees A's constants
 *     before top-level ones). `nesting` tracks the current dotted path.
 *   - Inheritance is `class C < Base`; mixins arrive via include/prepend/
 *     extend and participate in method lookup.
 *   - `Foo.new` constructs a Foo and dispatches to Foo#initialize.
 *   - `def m` defines an instance method; `def self.m` a singleton (class)
 *     method. Singleton methods are registered under the receiver key
 *     "<class_qn>.self" — constants cannot be lowercase, so the suffix can
 *     never collide with a real nested-constant path. */
typedef struct {
    CBMArena *arena;
    const char *source;
    int source_len;
    const CBMTypeRegistry *registry;
    /* Mutable alias of `registry`, set only while PASS 1 populates method
     * tables (mirrors perl_lsp's mutable-reg build phase). NULL afterwards. */
    CBMTypeRegistry *build_reg;
    CBMScope *current_scope;

    /* Lexical nesting: one frame per enclosing class/module declaration,
     * frames joined by '.', each frame keeping its raw source spelling ("" at
     * top level, "M.A::B" inside `module M; class A::B`). Ruby's constant
     * lookup walks the frames outward — inside `class A::B` the nesting is
     * [A::B] only, so A's constants are NOT probed. */
    const char *nesting;

    RubyAncestry *ancestry;

    /* Instance-variable type table (pass 1). */
    RubyIvarInfo *ivars;
    int ivar_count;
    int ivar_cap;

    /* Cross-file import map (require/require_relative resolved by the
     * pipeline). Unused per-file: Ruby constants are file-global. */
    const char **import_names;
    const char **import_qns;
    int import_count;

    /* Current enclosing context during the resolution walk. */
    const char *enclosing_class_qn; /* class QN, or NULL at top level */
    const char *enclosing_func_qn;  /* enclosing method QN, or NULL */
    bool in_singleton_method;       /* `def self.m` / class << self scope */
    /* Inside a block that rebinds `self` (instance_eval/exec, class_eval/
     * exec, module_eval/exec, define_method, Class.new/Module.new): the
     * receiver of bare and `self.` calls is NOT the enclosing class, and
     * the resolver cannot know what it is — so they emit nothing. */
    bool self_unknown;
    /* Number of enclosing blocks (`{ }`, `do end`, `-> { }`) since the
     * nearest class/module body. A `def` met here is NOT a method of the
     * enclosing class — ActiveSupport's `class_methods do … end`,
     * `included do … end`, RSpec groups — so PASS 1 registers nothing for
     * it and collects no mixins from it. */
    int block_depth;
    const char *module_qn;

    /* Output: resolved calls accumulate here. */
    CBMResolvedCallArray *resolved_calls;

    /* Recursion guards. */
    int eval_depth;
    int walk_depth;

    /* Debug mode (CBM_LSP_DEBUG env). */
    bool debug;
} RubyLSPContext;

/* Resolve a dotted constant path (e.g. "Foo", "A.B") against the class
 * table using the current lexical nesting, then the stdlib. Returns the
 * resolved QN or NULL. Borrowed until the ancestry is freed. */
const char *ruby_resolve_constant(RubyLSPContext *ctx, const char *path);

/* Evaluate a Ruby expression's type. May return CBM_TYPE_UNKNOWN. */
const CBMType *ruby_eval_expr_type(RubyLSPContext *ctx, TSNode node);

/* Entry point: build registry from file defs + stdlib, then run resolution.
 * Called from cbm_extract_file() via the language dispatch in cbm.c. */
void cbm_run_ruby_lsp(CBMArena *arena, CBMFileResult *result, const char *source, int source_len,
                      TSNode root);

/* Register Ruby core stdlib types/methods + a curated Rails surface
 * (ActiveRecord/ActiveSupport/ActionController) into a registry. */
void cbm_ruby_stdlib_register(CBMTypeRegistry *reg, CBMArena *arena);

#endif /* CBM_LSP_RUBY_LSP_H */
