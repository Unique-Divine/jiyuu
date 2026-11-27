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

    free(source_qn);
    free(target_qn);
    cbm_pipeline_set_pkgmap(NULL);
    cbm_pkgmap_free(pkgmap);
    cbm_gbuf_free(ctx.gbuf);
    free(project);
}
