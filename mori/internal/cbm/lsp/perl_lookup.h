#ifndef CBM_LSP_PERL_LOOKUP_H
#define CBM_LSP_PERL_LOOKUP_H

#include "type_registry.h"

typedef struct MoriPerlPackages MoriPerlPackages;

typedef struct {
    const CBMRegisteredFunc *(*exact)(const CBMTypeRegistry *, const char *qualified_name);
    const CBMRegisteredFunc *(*direct_method)(const CBMTypeRegistry *, const char *package,
                                              const char *name);
} MoriPerlRegistryLookup;

/* The index copies names. Registry callbacks borrow names for the duration of
 * the call and return entries owned by the C registry. */
MoriPerlPackages *mori_perl_packages_new(void);
void mori_perl_packages_free(MoriPerlPackages *packages);
void mori_perl_packages_add_parent(MoriPerlPackages *packages, const char *package,
                                   const char *parent);
void mori_perl_packages_add_sub(MoriPerlPackages *packages, const char *package, const char *name,
                                const char *module_qn);
const CBMRegisteredFunc *mori_perl_packages_lookup(const MoriPerlPackages *packages,
                                                   const CBMTypeRegistry *registry,
                                                   const MoriPerlRegistryLookup *lookup,
                                                   const char *package, const char *name,
                                                   bool super_call);

#endif
