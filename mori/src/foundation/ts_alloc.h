#ifndef CBM_TS_ALLOC_H
#define CBM_TS_ALLOC_H

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

/* Tree-sitter uses the result of a nonzero allocation without checking for NULL. */
static inline void *cbm_ts_alloc_or_abort(void *ptr, size_t count, size_t size) {
    if (!ptr && count != 0 && size != 0) {
        if (count > SIZE_MAX / size) {
            fprintf(stderr, "tree-sitter allocation size overflow (%zu x %zu bytes)\n", count,
                    size);
        } else {
            fprintf(stderr, "tree-sitter failed to allocate %zu bytes\n", count * size);
        }
        abort();
    }
    return ptr;
}

#endif
