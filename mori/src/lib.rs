//! Rust implementations and registration for the C graph engine.

#[path = "daemon/ipc/mod.rs"]
pub mod ipc;

#[cfg(feature = "native-engine-abi")]
#[doc(hidden)]
#[path = "cypher/cypher_scalar.rs"]
pub mod cypher_scalar;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "mcp/stdio/mod.rs"]
mod stdio;

#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/ansible/mod.rs"]
mod ansible;
#[cfg(feature = "native-engine-abi")]
#[path = "store/architecture_projection.rs"]
mod architecture_projection;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/closure_plan.rs"]
mod closure_plan;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/complexity/mod.rs"]
mod complexity;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/csharp_doc_links/mod.rs"]
mod csharp_doc_links;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/decorator_tags/mod.rs"]
mod decorator_tags;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/definition_props.rs"]
mod definition_props;
#[cfg(feature = "native-engine-abi")]
#[path = "mcp/detect_changes/mod.rs"]
mod detect_changes;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/diagrams.rs"]
mod diagrams;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/doc_file_links.rs"]
mod doc_file_links;
#[cfg(feature = "native-engine-abi")]
#[path = "store/doc_links_preview.rs"]
mod doc_links_preview;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/docker_image.rs"]
mod docker_image;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/explicit_override/mod.rs"]
mod explicit_override;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/extraction_schedule.rs"]
mod extraction_schedule;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "foundation/ffi_output.rs"]
mod ffi_output;
#[cfg(feature = "native-engine-abi")]
#[path = "graph_buffer/graph_buffer_merge.rs"]
mod graph_buffer_merge;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/jvm_doc_links/mod.rs"]
mod jvm_doc_links;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/kubernetes.rs"]
mod kubernetes;
#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lang_specs.rs"]
mod lang_specs;
#[cfg(feature = "native-engine-abi")]
#[path = "mcp/impact_graph.rs"]
mod mcp_impact_graph;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/msbuild_condition.rs"]
mod msbuild_condition;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/msbuild_expand.rs"]
mod msbuild_expand;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/msbuild_import.rs"]
mod msbuild_import;
#[cfg(feature = "native-engine-abi")]
#[path = "graph_buffer/native_graph_buffer.rs"]
mod native_graph_buffer;
#[cfg(feature = "native-engine-abi")]
#[path = "foundation/native_hash_table.rs"]
mod native_hash_table;
#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/native_parser.rs"]
mod native_parser;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/operator_helm.rs"]
mod operator_helm;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/pdf_doc_links.rs"]
mod pdf_doc_links;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/pkgmap/mod.rs"]
mod pkgmap;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/rust_doc_cargo.rs"]
mod rust_doc_cargo;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/rust_doc_path.rs"]
mod rust_doc_path;
#[cfg(feature = "native-engine-abi")]
#[path = "mcp/search/mod.rs"]
mod search;
#[cfg(feature = "native-engine-abi")]
#[path = "mcp/search_code_request/mod.rs"]
mod search_code_request;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/semantic_admission.rs"]
mod semantic_admission;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/semantic_candidates.rs"]
mod semantic_candidates;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/shell_pass.rs"]
mod shell_pass;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/similarity/mod.rs"]
mod similarity;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "mcp/snippet_window/mod.rs"]
mod snippet_window;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/swift_owner_index.rs"]
mod swift_owner_index;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/swift_package_map.rs"]
mod swift_package_map;
#[path = "pipeline/swift_signature_index.rs"]
mod swift_signature_index;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/terraform.rs"]
mod terraform;
#[cfg(feature = "native-engine-abi")]
#[path = "cli/test_impact_cli.rs"]
mod test_impact_cli;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "mcp/trace_page.rs"]
mod trace_page;

#[cfg(all(feature = "c-backend", not(test)))]
pub use search::register_search_readers;
#[cfg(feature = "native-engine-abi")]
#[path = "mcp/test_impact_engine_reach/mod.rs"]
mod test_impact_engine_reach;
#[cfg(feature = "native-engine-abi")]
#[path = "mcp/test_impact_git.rs"]
mod test_impact_git;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "cli/config_cli/mod.rs"]
mod config_cli;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "cli/tiered_profiles/mod.rs"]
mod tiered_profiles;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "mcp/mcp_dispatch/methods.rs"]
mod mcp_methods;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "mcp/project_listing/mod.rs"]
mod project_listing;

#[cfg(feature = "native-engine-abi")]
#[path = "mcp/project_response/mod.rs"]
mod project_response;

#[cfg(feature = "c-backend")]
pub use project_response::register_index_status_renderer;

#[cfg(feature = "native-engine-abi")]
#[path = "cli/startup.rs"]
pub mod startup;

#[cfg(feature = "native-engine-abi")]
#[path = "ui/http_host.rs"]
mod ui_http_host;

#[cfg(feature = "native-engine-abi")]
#[path = "ui/http_transport.rs"]
mod ui_http_transport;

#[cfg(feature = "native-engine-abi")]
#[path = "ui/xrepo.rs"]
mod ui_xrepo;

#[cfg(any(feature = "graphql-callbacks", test))]
#[path = "pipeline/gqlgen.rs"]
mod gqlgen;
#[cfg(any(all(feature = "graphql-callbacks", not(test)), test))]
#[path = "../internal/cbm/graphql_document.rs"]
mod graphql_document;
#[cfg(any(feature = "graphql-callbacks", test))]
#[path = "pipeline/graphql_link.rs"]
mod graphql_link;
#[cfg(any(feature = "graphql-callbacks", test))]
#[path = "pipeline/link.rs"]
mod link;
#[cfg(any(feature = "graphql-callbacks", test))]
#[path = "pipeline/route_writer.rs"]
mod route_writer;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "daemon/index_supervision/mod.rs"]
mod index_supervision;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "daemon/job_recovery/mod.rs"]
mod job_recovery;
#[cfg(feature = "native-engine-abi")]
#[path = "daemon/project_lock.rs"]
mod project_lock;
#[cfg(any(feature = "native-engine-abi", test))]
mod watcher;

#[cfg(all(feature = "graphql-callbacks", not(test)))]
#[path = "pipeline/graphql_ffi.rs"]
mod graphql_ffi;

#[cfg(all(feature = "graphql-callbacks", not(test)))]
pub use graphql_ffi::mori_register_graphql_callbacks;

#[cfg(all(feature = "c-backend", test))]
#[test]
fn rusqlite_uses_the_c_backend_sqlite() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let omit_load_extension: bool = db
        .query_row(
            "SELECT sqlite_compileoption_used('OMIT_LOAD_EXTENSION')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // The Mori amalgamation omits extension loading. The separately bundled
    // rusqlite archive includes it, so this checks which SQLite was linked.
    assert!(omit_load_extension);
}

#[cfg(all(feature = "c-backend", test, unix))]
#[path = "discover/discovery_spike/mod.rs"]
mod discovery_spike;

#[cfg(feature = "native-engine-abi")]
#[path = "foundation/abi_rows.rs"]
mod abi_rows;

#[cfg(feature = "native-engine-abi")]
#[path = "discover/discovery_spike/results.rs"]
mod discover_results;

#[cfg(feature = "native-engine-abi")]
#[path = "discover/discovery_spike/native.rs"]
mod discover_walk;

#[cfg(all(feature = "c-backend", unix))]
#[path = "store/staged_graph/mod.rs"]
mod staged_graph;

#[cfg(all(feature = "c-backend", not(test), unix))]
pub use staged_graph::native::register as mori_register_staged_graph_writer;

#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/index_manifest.rs"]
mod index_manifest;

#[cfg(feature = "c-backend")]
#[path = "../internal/cbm/graphql_extract.rs"]
mod graphql_extract;

#[cfg(feature = "c-backend")]
pub use graphql_extract::register as mori_register_graphql_extractor;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/go_cross.rs"]
mod go_cross;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/go_results.rs"]
mod go_results;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/resolver.rs"]
mod resolver;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/go_eval.rs"]
mod go_eval;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/go_identity.rs"]
mod go_identity;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/go_imports.rs"]
mod go_imports;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/go_registry_ast.rs"]
mod go_registry_ast;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/go_registry_defs.rs"]
mod go_registry_defs;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/go_registry_generics.rs"]
mod go_registry_generics;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/go_types.rs"]
mod go_types;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/go_types_native.rs"]
mod go_types_native;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/native_tree.rs"]
mod native_tree;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/native_type_registry.rs"]
mod native_type_registry;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/php_resolver/mod.rs"]
mod php_resolver;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/ruby_lookup.rs"]
mod ruby_lookup;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/node_helpers.rs"]
mod node_helpers;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/rust_path.rs"]
mod rust_path;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/c_field_lookup/mod.rs"]
mod c_field_lookup;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/perl_packages/mod.rs"]
mod perl_packages;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/ts_type_text.rs"]
mod ts_type_text;

#[cfg(feature = "c-backend")]
#[path = "../internal/cbm/rust_use_imports.rs"]
mod rust_use_imports;

#[cfg(feature = "c-backend")]
pub use rust_use_imports::register as mori_register_rust_use_imports;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/markdown_extract/mod.rs"]
mod markdown_extract;

#[cfg(feature = "c-backend")]
#[path = "../internal/cbm/ts_static_imports.rs"]
mod ts_static_imports;

#[cfg(feature = "c-backend")]
pub use ts_static_imports::register as mori_register_ts_static_imports;

#[cfg(feature = "native-engine-abi")]
#[path = "cli/activation_transaction/mod.rs"]
mod activation_transaction;
#[cfg(feature = "native-engine-abi")]
#[path = "cli/config_json_like/mod.rs"]
mod config_json_like;
#[cfg(feature = "native-engine-abi")]
#[path = "cli/config_text_edit/mod.rs"]
mod config_text_edit;
#[cfg(feature = "native-engine-abi")]
#[path = "cli/config_toml_edit/mod.rs"]
mod config_toml_edit;
#[cfg(feature = "native-engine-abi")]
#[path = "cli/config_yaml_edit/mod.rs"]
mod config_yaml_edit;
#[cfg(feature = "native-engine-abi")]
#[path = "mcp/file_outline/mod.rs"]
mod file_outline;
#[cfg(feature = "native-engine-abi")]
#[path = "store/fts_writer.rs"]
mod fts_writer;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "store/leiden.rs"]
mod leiden;
#[cfg(feature = "native-engine-abi")]
#[path = "store/store_digest/mod.rs"]
mod store_digest;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/full_build_driver.rs"]
mod full_build_driver;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/incremental_execute.rs"]
mod incremental_execute;
#[cfg(feature = "native-engine-abi")]
#[path = "pipeline/source_publish.rs"]
mod source_publish;
#[cfg(any(feature = "native-engine-abi", test))]
#[path = "pipeline/stage_publish.rs"]
mod stage_publish;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/python_annotation.rs"]
mod python_annotation;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/python_calls.rs"]
mod python_calls;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/python_mro.rs"]
mod python_mro;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "../internal/cbm/lsp/python_namespace.rs"]
mod python_namespace;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/lsp/python_walk.rs"]
mod python_walk;

#[cfg(feature = "native-engine-abi")]
#[path = "../internal/cbm/python_imports.rs"]
mod python_imports;

#[cfg(all(
    feature = "native-engine-abi",
    any(not(test), feature = "c-backend")
))]
#[path = "cli/hook_augment/mod.rs"]
mod hook_augment;
