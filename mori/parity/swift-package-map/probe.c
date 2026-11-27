/* Include the sequential edge writer so the fixture exercises its real IMPORTS output. */
#include "pipeline/pass_definitions.c"
#include <assert.h>

static void print_entries(const char *scan, int manifests, cbm_pkg_entries_t *entries) {
    printf("%s\t%d\t%d\n", scan, manifests, entries->count);
    for (int i = 0; i < entries->count; i++) {
        const cbm_pkg_entry_t *entry = &entries->items[i];
        printf("entry\t%s\t%s\t%s\t%s\t%d\n", entry->pkg_name ? entry->pkg_name : "<boundary>",
               entry->entry_rel, entry->manifest_rel ? entry->manifest_rel : "-",
               entry->source_rel ? entry->source_rel : "-", entry->source_seq);
    }
    cbm_pkg_entries_free(entries);
}

static char *file_qn(cbm_pipeline_ctx_t *ctx, const char *path, const char *name) {
    char *qn = cbm_pipeline_fqn_compute(ctx->project_name, path, "__file__");
    assert(qn);
    assert(cbm_gbuf_upsert_node(ctx->gbuf, "File", name, qn, path, 1, 1, "{}"));
    return qn;
}

static void check_import_selection(cbm_pipeline_ctx_t *ctx) {
    char *go_module = cbm_pipeline_resolve_module(ctx, "app.go", "example.com/demo/lib/tools");
    assert(go_module);
    const size_t project_len = strlen(ctx->project_name);
    assert(strncmp(go_module, ctx->project_name, project_len) == 0);
    assert(strcmp(go_module + project_len, ".lib.tools") == 0);
    printf("module\t%s\n", go_module);
    free(go_module);

    const char *php_source = "Php/Client.php";
    const char *php_provider = "Php/src/Thing.php";
    char *php_source_qn = file_qn(ctx, php_source, "Client.php");
    char *php_provider_qn = file_qn(ctx, php_provider, "Thing.php");
    const char *namespace_names[] = {"Z", "Z"};
    const char *namespace_paths[] = {php_source, php_provider};
    CBMHashTable *namespace_map = cbm_pipeline_namespace_map_build_names(
        ctx->project_name, namespace_names, namespace_paths, 2);
    assert(namespace_map);
    const char *bucket = cbm_ht_get(namespace_map, "Z");
    assert(bucket && strstr(bucket, php_source_qn) == bucket);
    const char *other = strchr(bucket, '\n');
    assert(other && strcmp(other + 1, php_provider_qn) == 0);

    CBMImport php[] = {
        {.module_path = "Z\\Thing", .local_name = "Thing"},
        {.module_path = "Z\\Missing", .local_name = "Missing"},
    };
    const cbm_gbuf_node_t *hit =
        cbm_pipeline_resolve_import_node(ctx, php_source, php_source_qn, &php[0], namespace_map);
    assert(hit && strcmp(hit->qualified_name, php_provider_qn) == 0);
    assert(
        !cbm_pipeline_resolve_import_node(ctx, php_source, php_source_qn, &php[1], namespace_map));
    printf("psr4\t%s\t%s\n", php_source_qn, hit->qualified_name);
    cbm_pipeline_namespace_map_free(namespace_map);
    free(php_source_qn);
    free(php_provider_qn);

    char *cargo = file_qn(ctx, "Workspace/Cargo.toml", "Cargo.toml");
    char *rust_source_qn = file_qn(ctx, "Workspace/src/lib.rs", "lib.rs");
    char *rust_target =
        cbm_pipeline_fqn_compute(ctx->project_name, "Workspace/src/known.rs", "known");
    assert(rust_target);
    assert(cbm_gbuf_upsert_node(ctx->gbuf, "Function", "known", rust_target,
                                "Workspace/src/known.rs", 1, 1, "{}"));
    CBMImport rust[] = {
        {.module_path = "crate::known", .local_name = "known"},
        {.module_path = "dependency::known", .local_name = "known"},
    };
    hit = cbm_pipeline_resolve_import_node(ctx, "Workspace/src/lib.rs", rust_source_qn, &rust[0],
                                           NULL);
    assert(hit && strcmp(hit->qualified_name, rust_target) == 0);
    assert(!cbm_pipeline_resolve_import_node(ctx, "Workspace/src/lib.rs", rust_source_qn, &rust[1],
                                             NULL));
    printf("rust\t%s\t%s\n", rust_source_qn, hit->qualified_name);
    free(cargo);
    free(rust_source_qn);
    free(rust_target);
}

int main(int argc, char **argv) {
    assert(argc == 2);
    cbm_log_set_level(CBM_LOG_NONE);
    const char *repo = argv[1];
    char *project = cbm_project_name_from_path(repo);
    printf("project\t%s\n", project);

    cbm_pkg_entries_t entries;
    char *excluded[] = {"Excluded"};
    cbm_pkg_entries_init(&entries);
    int manifests = cbm_pkgmap_scan_repo(repo, &entries, excluded, 1);
    assert(manifests == 14 && entries.count == 23);
    print_entries("repo", manifests, &entries);

    cbm_pkg_entries_init(&entries);
    manifests = cbm_pkgmap_scan_swift_targets(repo, &entries, excluded, 1);
    assert(manifests == 4 && entries.count == 12);
    print_entries("swift", manifests, &entries);

    char manifest_rel[] = "Core/Package.swift";
    char manifest_path[4096];
    int size = snprintf(manifest_path, sizeof(manifest_path), "%s/%s", repo, manifest_rel);
    assert(size > 0 && (size_t)size < sizeof(manifest_path));
    cbm_file_info_t file = {.path = manifest_path, .rel_path = manifest_rel};
    CBMHashTable *discovered = cbm_pkgmap_build_from_files(&file, 1, project);
    assert(discovered && cbm_ht_has(discovered, "Core"));
    printf("discovered\tCore\t%s\n", (const char *)cbm_ht_get(discovered, "Core"));
    cbm_pkgmap_free(discovered);

    cbm_pipeline_ctx_t ctx = {
        .project_name = project,
        .gbuf = cbm_gbuf_new(project, repo),
    };
    CBMHashTable *pkgmap = cbm_pkgmap_build_from_repo(repo, NULL, 0, project, excluded, 1);
    assert(ctx.gbuf && pkgmap);
    assert(cbm_ht_has(pkgmap, "js-fixture"));
    const char *psr4_root = cbm_ht_get(pkgmap, "Z\\");
    assert(psr4_root && strcmp(psr4_root, "Php/src") == 0);
    const char *go_module = cbm_ht_get(pkgmap, "example.com/demo");
    assert(go_module && strcmp(go_module, project) == 0);
    printf("selected\tZ\\\t%s\n", psr4_root);
    printf("selected\texample.com/demo\t%s\n", go_module);
    assert(!cbm_ht_has(pkgmap, "Alias") && !cbm_ht_has(pkgmap, "Phantom"));
    cbm_pipeline_set_pkgmap(pkgmap);

    const char *rel = "App/Sources/App/App.swift";
    const char *provider = "Core/Library";
    char *source_qn = cbm_pipeline_fqn_compute(project, rel, "__file__");
    char *target_qn = cbm_pipeline_fqn_folder(project, provider);
    int64_t source =
        cbm_gbuf_upsert_node(ctx.gbuf, "File", "App.swift", source_qn, rel, 1, 2, "{}");
    int64_t target =
        cbm_gbuf_upsert_node(ctx.gbuf, "Folder", "Library", target_qn, provider, 0, 0, "{}");
    assert(source && target);

    CBMImport imports[] = {
        {.module_path = "Core", .local_name = "Core"},
        {.module_path = "Alias", .local_name = "Alias"},
        {.module_path = "Phantom", .local_name = "Phantom"},
    };
    CBMFileResult result = {.imports = {.items = imports, .count = 3}};
    assert(create_import_edges_for_file(&ctx, &result, rel, NULL) == 1);
    const cbm_gbuf_edge_t **edges = NULL;
    int count = 0;
    cbm_gbuf_find_edges_by_source_type(ctx.gbuf, source, "IMPORTS", &edges, &count);
    assert(count == 1 && edges[0]->target_id == target);
    printf("edge\t%s\t%s\t%s\t%s\n", source_qn, edges[0]->type, target_qn,
           edges[0]->properties_json);

    check_import_selection(&ctx);

    free(source_qn);
    free(target_qn);
    cbm_pipeline_set_pkgmap(NULL);
    cbm_pkgmap_free(pkgmap);
    cbm_gbuf_free(ctx.gbuf);
    free(project);
}
