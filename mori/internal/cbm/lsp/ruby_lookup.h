#ifndef CBM_LSP_RUBY_LOOKUP_H
#define CBM_LSP_RUBY_LOOKUP_H

#include "type_registry.h"

typedef struct RubyAncestry RubyAncestry;

typedef enum {
    RUBY_MIXIN_INCLUDE,
    RUBY_MIXIN_PREPEND,
    RUBY_MIXIN_EXTEND,
} RubyMixinKind;

typedef enum {
    RUBY_LOOKUP_RECEIVER,
    RUBY_LOOKUP_SELF,
    RUBY_LOOKUP_SUPER,
} RubySearch;

typedef struct {
    const CBMRegisteredFunc *target;
    bool blocked;
    uint64_t steps;
} RubyMethodResult;

typedef const char *(*RubyTypeLookup)(const void *registry, const char *path);
typedef const CBMRegisteredFunc *(*RubyMethodLookup)(const void *registry, const char *receiver,
                                                     const char *name);

RubyAncestry *mori_ruby_ancestry_new(void);
void mori_ruby_ancestry_free(RubyAncestry *ancestry);
void mori_ruby_ancestry_declare(RubyAncestry *ancestry, const char *qn, const char *nesting,
                                const char *path, const char *parent);
void mori_ruby_ancestry_mixin(RubyAncestry *ancestry, const char *owner, const char *nesting,
                              const char *path, int kind);
void mori_ruby_ancestry_finalize(RubyAncestry *ancestry, const void *registry,
                                 RubyTypeLookup lookup);
/* Borrowed from ancestry. Copy to the file arena before retaining in an emitted row. */
const char *mori_ruby_ancestry_constant(const RubyAncestry *ancestry, const char *nesting,
                                        const char *path);
RubyMethodResult mori_ruby_ancestry_lookup(const RubyAncestry *ancestry, const void *registry,
                                           RubyMethodLookup lookup, const char *qn,
                                           const char *name, bool singleton, int search,
                                           uint32_t max_depth);
bool mori_ruby_ancestry_is_ar_model(const RubyAncestry *ancestry, const char *qn,
                                    uint32_t max_depth);
size_t mori_ruby_ancestry_counts(const RubyAncestry *ancestry, size_t *mixins);

#ifdef CBM_ENABLE_TEST_SEAMS
/* C reference for the Rust ancestry table (ruby_lsp.c). Setting
 * cbm_ruby_lookup_c_reference routes this thread's cbm_run_ruby_lsp calls to
 * it, so tests can compare whole-file results. */
extern _Thread_local bool cbm_ruby_lookup_c_reference;
RubyAncestry *cbm_ruby_ancestry_c_new(void);
void cbm_ruby_ancestry_c_free(RubyAncestry *ancestry);
void cbm_ruby_ancestry_c_declare(RubyAncestry *ancestry, const char *qn, const char *nesting,
                                 const char *path, const char *parent);
void cbm_ruby_ancestry_c_mixin(RubyAncestry *ancestry, const char *owner, const char *nesting,
                               const char *path, int kind);
void cbm_ruby_ancestry_c_finalize(RubyAncestry *ancestry, const void *registry,
                                  RubyTypeLookup lookup);
const char *cbm_ruby_ancestry_c_constant(const RubyAncestry *ancestry, const char *nesting,
                                         const char *path);
RubyMethodResult cbm_ruby_ancestry_c_lookup(const RubyAncestry *ancestry, const void *registry,
                                            RubyMethodLookup lookup, const char *qn,
                                            const char *name, bool singleton, int search,
                                            uint32_t max_depth);
bool cbm_ruby_ancestry_c_is_ar_model(const RubyAncestry *ancestry, const char *qn,
                                     uint32_t max_depth);
size_t cbm_ruby_ancestry_c_counts(const RubyAncestry *ancestry, size_t *mixins);
#endif

#endif
