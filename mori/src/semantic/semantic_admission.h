#ifndef CBM_SEMANTIC_ADMISSION_H
#define CBM_SEMANTIC_ADMISSION_H

#include <stdbool.h>
#include <stdint.h>

/* Shared with semantic_admission.rs. */
typedef struct {
    int64_t source_id;
    int64_t target_id;
    float score;
    bool same_file;
    bool below; /* recorded for pair signals only */
    int i;
    int j;
    int c; /* candidate rank for function i */
} cbm_semantic_pair_t;

/* Pairs are in canonical (i, c) order. counts has one slot per function.
 * order is scratch for n indices. admitted is written in canonical order. */
void mori_semantic_admit_best_first(const cbm_semantic_pair_t *pairs, int n, int max_edges,
                                   int *counts, int func_count, int *order, bool *admitted);

#endif
