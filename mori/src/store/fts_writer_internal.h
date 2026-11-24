#ifndef CBM_FTS_WRITER_INTERNAL_H
#define CBM_FTS_WRITER_INTERNAL_H

#include "store/store.h"

/* The writer borrows the live connection and the caller owns the transaction. */
typedef void (*cbm_fts_report_error_fn)(cbm_store_t *s, const char *context, const char *message);

int mori_fts_write(cbm_store_t *s, struct sqlite3 *db, const char *project, int64_t after_id,
                   cbm_fts_report_error_fn report_error);

#endif
