#ifndef CBM_LSP_PERL_LSP_H
#define CBM_LSP_PERL_LSP_H

#include "type_registry.h"
#include "../cbm.h"

/* Entry point: build registry from file defs + stdlib, then run resolution.
 * Called from cbm_extract_file() via the language dispatch in cbm.c. */
void cbm_run_perl_lsp(CBMArena *arena, CBMFileResult *result, const char *source, int source_len,
                      TSNode root);

/* Register Perl stdlib (perlfunc builtins) + curated CPAN types into a
 * registry. */
void cbm_perl_stdlib_register(CBMTypeRegistry *reg, CBMArena *arena);

#endif /* CBM_LSP_PERL_LSP_H */
