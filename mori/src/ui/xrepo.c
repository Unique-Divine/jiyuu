/*
 * xrepo.c — Cross-repo summary and link listing over several project
 * databases (GET /api/xrepo/summary, GET /api/xrepo/links). See xrepo.h.
 */
#include "ui/xrepo.h"
#include "store/store.h"
#include "foundation/compat_fs.h"
#include "foundation/json_writer.h"
#include "foundation/platform.h"
#include "foundation/str_util.h"

#include <sqlite3/sqlite3.h>
#include <yyjson/yyjson.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Bounds: projects read per summary, and cache entries scanned for them. */
#define XREPO_MAX_PROJECTS 512
#define XREPO_MAX_ENTRIES 20000
#define XREPO_NAME_MAX 256

/* CROSS_* types sort between "CROSS_" and "CROSS`" ('`' follows '_'), so the
 * (project, type) index answers the range. */
#define XREPO_TYPE_RANGE "e.type >= 'CROSS_' AND e.type < 'CROSS`'"

/* The caller-side copy of a link. Route-style links (HTTP, async, gRPC,
 * GraphQL, tRPC) carry a "callers" object only on the provider's copy.
 * Channel links carry none; the emitter's copy is the one whose source
 * EMITS into the same channel node. */
#define XREPO_CALLER_SIDE                                                              \
    "(CASE WHEN e.type = 'CROSS_CHANNEL' THEN EXISTS (SELECT 1 FROM edges x "         \
    "WHERE x.source_id = e.source_id AND x.target_id = e.target_id AND x.type = "     \
    "'EMITS') ELSE json_type(e.properties, '$.callers') IS NULL END)"

#define XREPO_TARGET "coalesce(json_extract(e.properties, '$.target_project'), '')"

/* The property that names what a link goes through, most specific first. */
#define XREPO_VIA                                                                        \
    "coalesce(json_extract(e.properties, '$.operation'), "                              \
    "json_extract(e.properties, '$.procedure'), json_extract(e.properties, '$.url_path'), " \
    "json_extract(e.properties, '$.channel_name'), json_extract(e.properties, '$.service'), " \
    "'')"

typedef struct {
    char name[XREPO_NAME_MAX];
    int64_t nodes;
    int64_t edges;
    int64_t links_out;
    int64_t links_in;
    bool indexed;
} xrepo_repo_t;

typedef struct {
    char from[XREPO_NAME_MAX];
    char to[XREPO_NAME_MAX];
    char type[64];
    int64_t count;
} xrepo_link_t;

typedef struct {
    xrepo_repo_t *repos;
    int repo_count;
    int repo_cap;
    xrepo_link_t *links;
    int link_count;
    int link_cap;
} xrepo_acc_t;

bool cbm_xrepo_is_cross_type(const char *type) {
    static const char prefix[] = "CROSS_";
    if (!type || strncmp(type, prefix, sizeof(prefix) - 1) != 0)
        return false;
    const char *p = type + sizeof(prefix) - 1;
    if (!*p || strlen(type) >= sizeof(((xrepo_link_t *)0)->type))
        return false;
    for (; *p; p++) {
        if (!((*p >= 'A' && *p <= 'Z') || *p == '_'))
            return false;
    }
    return true;
}

static bool grow(void **items, int *cap, int count, size_t size) {
    if (count < *cap)
        return true;
    int next = *cap ? *cap * 2 : 16;
    void *grown = cbm_realloc(CBM_MEM_CLASS_OTHER, *items, (size_t)next * size);
    if (!grown)
        return false;
    *items = grown;
    *cap = next;
    return true;
}

static xrepo_repo_t *repo_find_or_add(xrepo_acc_t *acc, const char *name) {
    for (int i = 0; i < acc->repo_count; i++) {
        if (strcmp(acc->repos[i].name, name) == 0)
            return &acc->repos[i];
    }
    if (!grow((void **)&acc->repos, &acc->repo_cap, acc->repo_count, sizeof(xrepo_repo_t)))
        return NULL;
    xrepo_repo_t *repo = &acc->repos[acc->repo_count++];
    memset(repo, 0, sizeof(*repo));
    snprintf(repo->name, sizeof(repo->name), "%s", name);
    return repo;
}

static bool link_add(xrepo_acc_t *acc, const char *from, const char *to, const char *type,
                     int64_t count) {
    if (!grow((void **)&acc->links, &acc->link_cap, acc->link_count, sizeof(xrepo_link_t)))
        return false;
    xrepo_link_t *link = &acc->links[acc->link_count++];
    memset(link, 0, sizeof(*link));
    snprintf(link->from, sizeof(link->from), "%s", from);
    snprintf(link->to, sizeof(link->to), "%s", to);
    snprintf(link->type, sizeof(link->type), "%s", type);
    link->count = count;
    return true;
}

static void db_path_in(const char *cache_dir, const char *project, char *buf, size_t cap) {
    snprintf(buf, cap, "%s/%s.db", cache_dir, project);
}

/* Read one project's counts and caller-side links into acc. One read
 * transaction keeps the counts and links on one WAL snapshot; WAL readers
 * never block the indexer's writer. */
static bool read_project(xrepo_acc_t *acc, const char *cache_dir, const char *project) {
    char path[1024];
    db_path_in(cache_dir, project, path, sizeof(path));
    cbm_store_t *store = cbm_store_open_path_query(path);
    if (!store)
        return false;
    sqlite3 *db = cbm_store_get_db(store);
    bool ok = db && sqlite3_exec(db, "BEGIN", NULL, NULL, NULL) == SQLITE_OK;
    sqlite3_stmt *st = NULL;
    int64_t counts[3] = {0};
    /* Other stores share the cache directory (activity.db, for one); only a
     * database with the index tables can hold a project. */
    bool has_tables = false;
    if (ok && sqlite3_prepare_v2(db,
                                 "SELECT count(*) = 3 FROM sqlite_master WHERE type = 'table' "
                                 "AND name IN ('projects', 'nodes', 'edges')",
                                 -1, &st, NULL) == SQLITE_OK &&
        sqlite3_step(st) == SQLITE_ROW)
        has_tables = sqlite3_column_int(st, 0) != 0;
    sqlite3_finalize(st);
    st = NULL;
    ok = ok && has_tables &&
         sqlite3_prepare_v2(db,
                            "SELECT (SELECT count(*) FROM projects WHERE name = ?1), "
                            "(SELECT count(*) FROM nodes WHERE project = ?1), "
                            "(SELECT count(*) FROM edges WHERE project = ?1)",
                            -1, &st, NULL) == SQLITE_OK &&
         sqlite3_bind_text(st, 1, project, -1, SQLITE_STATIC) == SQLITE_OK &&
         sqlite3_step(st) == SQLITE_ROW;
    for (int i = 0; ok && i < 3; i++)
        counts[i] = sqlite3_column_int64(st, i);
    sqlite3_finalize(st);
    st = NULL;
    /* A database without its own project row (an internal store that
     * happens to end in .db) is not a repository: leave it out. */
    bool is_project = ok && counts[0] > 0;
    xrepo_repo_t *repo = is_project ? repo_find_or_add(acc, project) : NULL;
    ok = ok && (!is_project || repo != NULL);
    if (repo) {
        repo->indexed = true;
        repo->nodes = counts[1];
        repo->edges = counts[2];
    }
    if (ok && is_project) {
        ok = sqlite3_prepare_v2(db,
                                "SELECT e.type, " XREPO_TARGET ", count(*) FROM edges e "
                                "WHERE e.project = ?1 AND " XREPO_TYPE_RANGE
                                " AND " XREPO_CALLER_SIDE " GROUP BY 1, 2",
                                -1, &st, NULL) == SQLITE_OK &&
             sqlite3_bind_text(st, 1, project, -1, SQLITE_STATIC) == SQLITE_OK;
        int rc = SQLITE_DONE;
        while (ok && (rc = sqlite3_step(st)) == SQLITE_ROW) {
            const char *type = (const char *)sqlite3_column_text(st, 0);
            const char *target = (const char *)sqlite3_column_text(st, 1);
            int64_t count = sqlite3_column_int64(st, 2);
            if (!type || !target || !target[0] || strcmp(target, project) == 0 ||
                !cbm_validate_project_name(target))
                continue;
            ok = link_add(acc, project, target, type, count);
        }
        ok = ok && rc == SQLITE_DONE;
        sqlite3_finalize(st);
    }
    if (db)
        sqlite3_exec(db, "COMMIT", NULL, NULL, NULL);
    cbm_store_close(store);
    return ok || (db && !has_tables);
}

/* Project names (file stems) of the project indexes in cache_dir, sorted. */
static int list_projects(const char *cache_dir, char (**out)[XREPO_NAME_MAX]) {
    *out = NULL;
    cbm_dir_t *d = cbm_opendir(cache_dir);
    if (!d)
        return -1;
    char (*names)[XREPO_NAME_MAX] = NULL;
    int count = 0, cap = 0, scanned = 0;
    cbm_dirent_t *ent;
    while ((ent = cbm_readdir(d)) != NULL && ++scanned <= XREPO_MAX_ENTRIES &&
           count < XREPO_MAX_PROJECTS) {
        if (!cbm_is_project_index_db(ent->name))
            continue;
        size_t stem = strlen(ent->name) - 3U; /* ".db" */
        if (stem == 0 || stem >= XREPO_NAME_MAX)
            continue;
        if (!grow((void **)&names, &cap, count, sizeof(*names))) {
            cbm_closedir(d);
            cbm_free(CBM_MEM_CLASS_OTHER, names);
            return -1;
        }
        memcpy(names[count], ent->name, stem);
        names[count][stem] = '\0';
        if (cbm_validate_project_name(names[count]))
            count++;
    }
    cbm_closedir(d);
    qsort(names, (size_t)count, sizeof(*names), (int (*)(const void *, const void *))strcmp);
    *out = names;
    return count;
}

static int link_order(const void *a, const void *b) {
    const xrepo_link_t *x = a, *y = b;
    if (x->count != y->count)
        return x->count > y->count ? -1 : 1;
    int c = strcmp(x->from, y->from);
    if (c)
        return c;
    c = strcmp(x->to, y->to);
    return c ? c : strcmp(x->type, y->type);
}

static char *write_doc(yyjson_mut_doc *doc) {
    char *json = yyjson_mut_write_opts(doc, 0, cbm_json_writer_allocator(), NULL, NULL);
    yyjson_mut_doc_free(doc);
    return json;
}

char *cbm_xrepo_summary_json(const char *cache_dir) {
    if (!cache_dir)
        return NULL;
    char (*names)[XREPO_NAME_MAX] = NULL;
    int count = list_projects(cache_dir, &names);
    if (count < 0)
        return NULL;
    xrepo_acc_t acc = {0};
    yyjson_mut_doc *doc = yyjson_mut_doc_new(NULL);
    yyjson_mut_val *root = doc ? yyjson_mut_obj(doc) : NULL;
    yyjson_mut_val *skipped = doc ? yyjson_mut_arr(doc) : NULL;
    char *json = NULL;
    if (!root || !skipped)
        goto done;
    yyjson_mut_doc_set_root(doc, root);
    for (int i = 0; i < count; i++) {
        if (!read_project(&acc, cache_dir, names[i]))
            yyjson_mut_arr_add_strcpy(doc, skipped, names[i]);
    }
    /* Totals per repo; a target with no index of its own still gets a node. */
    for (int i = 0; i < acc.link_count; i++) {
        xrepo_repo_t *from = repo_find_or_add(&acc, acc.links[i].from);
        if (from)
            from->links_out += acc.links[i].count;
        xrepo_repo_t *to = repo_find_or_add(&acc, acc.links[i].to);
        if (to)
            to->links_in += acc.links[i].count;
    }
    if (acc.links)
        qsort(acc.links, (size_t)acc.link_count, sizeof(*acc.links), link_order);
    yyjson_mut_val *repos = yyjson_mut_arr(doc);
    yyjson_mut_val *links = yyjson_mut_arr(doc);
    yyjson_mut_val *types = yyjson_mut_arr(doc);
    for (int i = 0; i < acc.repo_count; i++) {
        const xrepo_repo_t *r = &acc.repos[i];
        yyjson_mut_val *o = yyjson_mut_arr_add_obj(doc, repos);
        yyjson_mut_obj_add_strcpy(doc, o, "name", r->name);
        yyjson_mut_obj_add_bool(doc, o, "indexed", r->indexed);
        yyjson_mut_obj_add_int(doc, o, "nodes", r->nodes);
        yyjson_mut_obj_add_int(doc, o, "edges", r->edges);
        yyjson_mut_obj_add_int(doc, o, "links_out", r->links_out);
        yyjson_mut_obj_add_int(doc, o, "links_in", r->links_in);
    }
    int64_t total = 0;
    for (int i = 0; i < acc.link_count; i++) {
        const xrepo_link_t *l = &acc.links[i];
        yyjson_mut_val *o = yyjson_mut_arr_add_obj(doc, links);
        yyjson_mut_obj_add_strcpy(doc, o, "from", l->from);
        yyjson_mut_obj_add_strcpy(doc, o, "to", l->to);
        yyjson_mut_obj_add_strcpy(doc, o, "type", l->type);
        yyjson_mut_obj_add_int(doc, o, "count", l->count);
        total += l->count;
        bool seen = false;
        for (int j = 0; j < i && !seen; j++)
            seen = strcmp(acc.links[j].type, l->type) == 0;
        if (!seen)
            yyjson_mut_arr_add_strcpy(doc, types, l->type);
    }
    yyjson_mut_obj_add_val(doc, root, "repos", repos);
    yyjson_mut_obj_add_val(doc, root, "links", links);
    yyjson_mut_obj_add_val(doc, root, "types", types);
    yyjson_mut_obj_add_int(doc, root, "total_links", total);
    yyjson_mut_obj_add_val(doc, root, "skipped", skipped);
    yyjson_mut_obj_add_str(doc, root, "scope",
                           "caller-side CROSS_* edges stored by the cross-repo pass");
    json = write_doc(doc);
    doc = NULL;
done:
    if (doc)
        yyjson_mut_doc_free(doc);
    cbm_free(CBM_MEM_CLASS_OTHER, acc.repos);
    cbm_free(CBM_MEM_CLASS_OTHER, acc.links);
    cbm_free(CBM_MEM_CLASS_OTHER, names);
    return json;
}

static void add_text(yyjson_mut_doc *doc, yyjson_mut_val *obj, const char *key,
                     sqlite3_stmt *st, int col) {
    const char *value = (const char *)sqlite3_column_text(st, col);
    yyjson_mut_obj_add_strcpy(doc, obj, key, value ? value : "");
}

static void bind_optional(sqlite3_stmt *st, int index, const char *value) {
    if (value && value[0])
        sqlite3_bind_text(st, index, value, -1, SQLITE_STATIC);
    else
        sqlite3_bind_null(st, index);
}

char *cbm_xrepo_links_json(const char *cache_dir, const char *from, const char *to,
                           const char *type, int limit, int offset, int *status) {
    int unused = 0;
    if (!status)
        status = &unused;
    *status = 400;
    if (!cache_dir || !from || !cbm_validate_project_name(from) ||
        (to && to[0] && !cbm_validate_project_name(to)) ||
        (type && type[0] && !cbm_xrepo_is_cross_type(type)))
        return NULL;
    if (limit <= 0)
        limit = CBM_XREPO_LINKS_DEFAULT_LIMIT;
    if (limit > CBM_XREPO_LINKS_MAX_LIMIT)
        limit = CBM_XREPO_LINKS_MAX_LIMIT;
    if (offset < 0)
        offset = 0;
    char path[1024];
    db_path_in(cache_dir, from, path, sizeof(path));
    *status = 404;
    if (!cbm_file_exists(path))
        return NULL;
    *status = 500;
    cbm_store_t *store = cbm_store_open_path_query(path);
    if (!store)
        return NULL;
    sqlite3 *db = cbm_store_get_db(store);
    yyjson_mut_doc *doc = yyjson_mut_doc_new(NULL);
    yyjson_mut_val *root = doc ? yyjson_mut_obj(doc) : NULL;
    sqlite3_stmt *st = NULL;
    char *json = NULL;
    bool began = db && sqlite3_exec(db, "BEGIN", NULL, NULL, NULL) == SQLITE_OK;
    if (!root || !began)
        goto done;
    yyjson_mut_doc_set_root(doc, root);
    yyjson_mut_obj_add_strcpy(doc, root, "from", from);
    yyjson_mut_obj_add_strcpy(doc, root, "to", to ? to : "");
    yyjson_mut_obj_add_strcpy(doc, root, "type", type ? type : "");
    yyjson_mut_obj_add_int(doc, root, "limit", limit);
    yyjson_mut_obj_add_int(doc, root, "offset", offset);

#define XREPO_LINK_FILTER                                                                 \
    " FROM edges e WHERE e.project = ?1 AND " XREPO_TYPE_RANGE " AND " XREPO_CALLER_SIDE \
    " AND (?2 IS NULL OR " XREPO_TARGET " = ?2) AND (?3 IS NULL OR e.type = ?3)"
    if (sqlite3_prepare_v2(db, "SELECT count(*)" XREPO_LINK_FILTER, -1, &st, NULL) != SQLITE_OK)
        goto done;
    sqlite3_bind_text(st, 1, from, -1, SQLITE_STATIC);
    bind_optional(st, 2, to);
    bind_optional(st, 3, type);
    if (sqlite3_step(st) != SQLITE_ROW)
        goto done;
    yyjson_mut_obj_add_int(doc, root, "total", sqlite3_column_int64(st, 0));
    sqlite3_finalize(st);
    st = NULL;

    if (sqlite3_prepare_v2(db,
                           "SELECT e.type, " XREPO_TARGET ", " XREPO_VIA ", "
                           "coalesce(json_extract(e.properties, '$.method'), ''), "
                           "coalesce(json_extract(e.properties, '$.target_function'), ''), "
                           "coalesce(json_extract(e.properties, '$.target_file'), ''), "
                           "s.name, s.qualified_name, s.label, s.file_path, s.start_line, "
                           "t.name, t.qualified_name, t.label "
                           "FROM edges e JOIN nodes s ON s.id = e.source_id "
                           "JOIN nodes t ON t.id = e.target_id "
                           "WHERE e.project = ?1 AND " XREPO_TYPE_RANGE " AND " XREPO_CALLER_SIDE
                           " AND (?2 IS NULL OR " XREPO_TARGET " = ?2) "
                           "AND (?3 IS NULL OR e.type = ?3) "
                           "ORDER BY e.type, 2, s.file_path, s.start_line, e.id "
                           "LIMIT ?4 OFFSET ?5",
                           -1, &st, NULL) != SQLITE_OK)
        goto done;
    sqlite3_bind_text(st, 1, from, -1, SQLITE_STATIC);
    bind_optional(st, 2, to);
    bind_optional(st, 3, type);
    sqlite3_bind_int(st, 4, limit);
    sqlite3_bind_int(st, 5, offset);
    yyjson_mut_val *links = yyjson_mut_arr(doc);
    int rc;
    while ((rc = sqlite3_step(st)) == SQLITE_ROW) {
        yyjson_mut_val *o = yyjson_mut_arr_add_obj(doc, links);
        add_text(doc, o, "type", st, 0);
        add_text(doc, o, "to", st, 1);
        add_text(doc, o, "via", st, 2);
        add_text(doc, o, "method", st, 3);
        add_text(doc, o, "target_function", st, 4);
        add_text(doc, o, "target_file", st, 5);
        yyjson_mut_val *source = yyjson_mut_obj_add_obj(doc, o, "source");
        add_text(doc, source, "name", st, 6);
        add_text(doc, source, "qualified_name", st, 7);
        add_text(doc, source, "label", st, 8);
        add_text(doc, source, "file_path", st, 9);
        yyjson_mut_obj_add_int(doc, source, "line", sqlite3_column_int64(st, 10));
        yyjson_mut_val *local = yyjson_mut_obj_add_obj(doc, o, "local_target");
        add_text(doc, local, "name", st, 11);
        add_text(doc, local, "qualified_name", st, 12);
        add_text(doc, local, "label", st, 13);
    }
    if (rc != SQLITE_DONE)
        goto done;
    yyjson_mut_obj_add_val(doc, root, "links", links);
    json = write_doc(doc);
    doc = NULL;
    *status = 200;
done:
    sqlite3_finalize(st);
    if (began)
        sqlite3_exec(db, "COMMIT", NULL, NULL, NULL);
    if (doc)
        yyjson_mut_doc_free(doc);
    cbm_store_close(store);
    return json;
}
