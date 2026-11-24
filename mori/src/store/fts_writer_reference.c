/* The standalone C executable and C tests retain this reference writer. */
#include "store/fts_writer_internal.h"

#include <sqlite3.h>
#include <stdio.h>

/* Prose source for nodes_fts.body: the docstring each node already carries in
 * its properties JSON.  Deriving it costs no new column on `nodes` and no new
 * table, which is exactly why CBM_INDEX_FORMAT_VERSION does not move and no
 * user is forced to reindex.
 *   - the LIKE prefilter keeps the JSON parse off the large majority of nodes
 *     that have no docstring at all;
 *   - json_valid() guards the parse because json_extract() RAISES on malformed
 *     properties, and pre-fix databases contain such rows (see the reverted
 *     is_entry_point expression index in create_user_indexes) — unguarded, one
 *     bad row would abort the entire backfill. */
#define FTS_BODY_EXPR                                                         \
    "CASE WHEN properties LIKE '%\"docstring\"%' AND json_valid(properties) " \
    "THEN json_extract(properties, '$.docstring') END"

/* nodes_fts.qualified_name: the QN without the C-macro fence. A macro's QN is
 * `<module>.NAME#macro` (CBM_MACRO_QN_SUFFIX in internal/cbm/cbm.h), and `#`
 * separates tokens, so the fence would add the word "macro" to every macro
 * node a second time (the label column already says Macro). On a C repository
 * that is thousands of rows outscoring the few nodes that carry the word in
 * their NAME, enough to push those out of the BM25 candidate window. */
#define FTS_QN_EXPR                                                        \
    "CASE WHEN label = 'Macro' AND substr(qualified_name, -6) = '#macro' " \
    "THEN substr(qualified_name, 1, length(qualified_name) - 6) "          \
    "ELSE qualified_name END"

enum { FTS_SQL_BUF = 768 };

/* Does nodes_fts carry the `body` column?  A database created by an older
 * build has only the four identifier columns, and CREATE VIRTUAL TABLE IF NOT
 * EXISTS does not widen it.  Probe rather than assume: naming a column that
 * is not there would fail the backfill outright, and the contract for a legacy
 * database is "still searchable, just without prose". */
static bool fts_has_body_column(sqlite3 *db) {
    sqlite3_stmt *probe = NULL;
    if (sqlite3_prepare_v2(db, "SELECT body FROM nodes_fts LIMIT 0;", -1, &probe, NULL) !=
        SQLITE_OK) {
        return false;
    }
    sqlite3_finalize(probe);
    return true;
}

static bool fts_build_query(char *sql, size_t capacity, bool partial, bool with_body, bool camel) {
    int n = snprintf(sql, capacity,
                     "INSERT INTO nodes_fts (rowid, name, qualified_name, label, file_path%s)"
                     " SELECT id, %s, " FTS_QN_EXPR ", label, file_path%s FROM nodes%s;",
                     with_body ? ", body" : "", camel ? "cbm_camel_split(name)" : "name",
                     with_body ? ", " FTS_BODY_EXPR : "",
                     partial ? " WHERE project = ?1 AND id > ?2" : "");
    return n > 0 && (size_t)n < capacity;
}

/* One backfill attempt.  CBM_STORE_NOT_FOUND means the statement would not
 * even prepare (missing table, column or SQL function) so the caller may retry
 * with a narrower shape; CBM_STORE_ERR means the write itself failed. */
static int fts_backfill_try(cbm_store_t *s, sqlite3 *db, const char *project, int64_t after_id,
                            bool with_body, bool camel, cbm_fts_report_error_fn report_error) {
    char sql[FTS_SQL_BUF];
    if (!fts_build_query(sql, sizeof(sql), project != NULL, with_body, camel)) {
        return CBM_STORE_ERR;
    }
    sqlite3_stmt *stmt = NULL;
    if (sqlite3_prepare_v2(db, sql, -1, &stmt, NULL) != SQLITE_OK) {
        return CBM_STORE_NOT_FOUND;
    }
    if (project) {
        sqlite3_bind_text(stmt, 1, project, -1, SQLITE_STATIC);
        sqlite3_bind_int64(stmt, 2, after_id);
    }
    int rc = sqlite3_step(stmt) == SQLITE_DONE ? CBM_STORE_OK : CBM_STORE_ERR;
    if (rc != CBM_STORE_OK) {
        report_error(s, "fts_backfill", sqlite3_errmsg(db));
    }
    sqlite3_finalize(stmt);
    return rc;
}

int cbm_store_fts_write_reference(cbm_store_t *s, sqlite3 *db, const char *project,
                                  int64_t after_id, cbm_fts_report_error_fn report_error) {
    /* Wholesale only: the incremental caller adds rows to a live index. */
    if (!project) {
        char *message = NULL;
        if (sqlite3_exec(db, "INSERT INTO nodes_fts(nodes_fts) VALUES('delete-all');", NULL, NULL,
                         &message) != SQLITE_OK) {
            report_error(s, "exec", message);
            sqlite3_free(message);
            return CBM_STORE_ERR;
        }
    }
    /* Degrade one capability at a time, widest first.  Dropping `body` covers a
     * legacy four-column table or a build without JSON1; dropping
     * cbm_camel_split covers a connection where the function is not registered
     * (the pre-existing fallback).  Each attempt is a single statement, so a
     * failed one rolls itself back before the next runs. */
    const bool with_body = fts_has_body_column(db);
    const struct {
        bool body;
        bool camel;
    } ladder[] = {{true, true}, {true, false}, {false, true}, {false, false}};
    int rc = CBM_STORE_NOT_FOUND;
    for (size_t i = 0; i < sizeof(ladder) / sizeof(ladder[0]); i++) {
        if (ladder[i].body && !with_body) {
            continue;
        }
        rc = fts_backfill_try(s, db, project, after_id, ladder[i].body, ladder[i].camel,
                              report_error);
        if (rc == CBM_STORE_OK) {
            return rc;
        }
    }
    return rc;
}
