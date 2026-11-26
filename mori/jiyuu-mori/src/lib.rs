//! Rust implementations and registration for the C graph engine.

pub mod ipc;

#[cfg(feature = "native-engine-abi")]
mod ansible;
#[cfg(feature = "native-engine-abi")]
mod architecture_projection;
#[cfg(feature = "native-engine-abi")]
mod closure_plan;
#[cfg(any(feature = "native-engine-abi", test))]
mod complexity;
#[cfg(feature = "native-engine-abi")]
mod csharp_doc_links;
#[cfg(feature = "native-engine-abi")]
mod doc_file_links;
#[cfg(feature = "native-engine-abi")]
mod docker_image;
#[cfg(any(feature = "native-engine-abi", test))]
mod explicit_override;
#[cfg(feature = "native-engine-abi")]
mod ffi_output;
#[cfg(feature = "native-engine-abi")]
mod jvm_doc_links;
#[cfg(feature = "native-engine-abi")]
mod kubernetes;
#[cfg(feature = "native-engine-abi")]
mod msbuild_condition;
#[cfg(feature = "native-engine-abi")]
mod native_graph_buffer;
#[cfg(feature = "native-engine-abi")]
mod native_hash_table;
#[cfg(any(feature = "native-engine-abi", test))]
mod operator_helm;
#[cfg(feature = "native-engine-abi")]
mod pdf_doc_links;
#[cfg(feature = "native-engine-abi")]
mod rust_doc_cargo;
#[cfg(feature = "native-engine-abi")]
mod rust_doc_path;
#[cfg(any(feature = "native-engine-abi", test))]
mod semantic_candidates;
#[cfg(feature = "native-engine-abi")]
mod shell_pass;
#[cfg(any(feature = "native-engine-abi", test))]
mod similarity;
#[cfg(any(feature = "native-engine-abi", test))]
mod swift_package_map;
#[cfg(feature = "native-engine-abi")]
mod terraform;
#[cfg(feature = "native-engine-abi")]
mod test_impact_git;

#[cfg(any(feature = "native-engine-abi", test))]
mod config_cli;

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "mcp_dispatch/methods.rs"]
mod mcp_methods;

#[cfg(any(feature = "native-engine-abi", test))]
mod project_listing;

#[cfg(feature = "native-engine-abi")]
mod project_response;

#[cfg(feature = "c-backend")]
pub use project_response::register_index_status_renderer;

#[cfg(feature = "native-engine-abi")]
pub mod startup;

#[cfg(feature = "native-engine-abi")]
#[path = "ui/http_transport.rs"]
mod ui_http_transport;

#[cfg(any(feature = "graphql-callbacks", test))]
mod gqlgen;
#[cfg(any(all(feature = "graphql-callbacks", not(test)), test))]
mod graphql_document;
#[cfg(any(feature = "graphql-callbacks", test))]
mod graphql_link;
#[cfg(any(feature = "graphql-callbacks", test))]
mod link;
#[cfg(any(feature = "graphql-callbacks", test))]
mod route_writer;

#[cfg(any(feature = "native-engine-abi", test))]
mod job_recovery;
#[cfg(any(feature = "native-engine-abi", test))]
mod watcher;

#[cfg(all(feature = "graphql-callbacks", not(test)))]
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
mod discovery_spike;

#[cfg(feature = "native-engine-abi")]
#[path = "discovery_spike/native.rs"]
mod discover_walk;

#[cfg(all(feature = "c-backend", unix))]
mod staged_graph;

#[cfg(all(feature = "c-backend", not(test), unix))]
pub use staged_graph::native::register as mori_register_staged_graph_writer;

#[cfg(all(feature = "c-backend", test))]
mod index_manifest;

#[cfg(feature = "c-backend")]
mod graphql_extract;

#[cfg(feature = "c-backend")]
pub use graphql_extract::register as mori_register_graphql_extractor;

#[cfg(feature = "c-backend")]
mod go_imports;

#[cfg(feature = "c-backend")]
pub use go_imports::register as mori_register_go_import_extractor;

#[cfg(feature = "c-backend")]
mod native_tree;

#[cfg(feature = "c-backend")]
mod rust_use_imports;

#[cfg(feature = "c-backend")]
pub use rust_use_imports::register as mori_register_rust_use_imports;

#[cfg(feature = "c-backend")]
mod markdown_extract;

#[cfg(feature = "c-backend")]
pub use markdown_extract::register as mori_register_markdown_extractor;

#[cfg(feature = "c-backend")]
mod ts_static_imports;

#[cfg(feature = "c-backend")]
pub use ts_static_imports::register as mori_register_ts_static_imports;

#[cfg(feature = "native-engine-abi")]
mod activation_transaction;
#[cfg(feature = "native-engine-abi")]
mod config_json_like;
#[cfg(feature = "native-engine-abi")]
mod config_text_edit;
#[cfg(feature = "native-engine-abi")]
mod config_toml_edit;
#[cfg(feature = "native-engine-abi")]
mod config_yaml_edit;
#[cfg(feature = "native-engine-abi")]
mod file_outline;
#[cfg(feature = "native-engine-abi")]
mod fts_writer;
#[cfg(feature = "native-engine-abi")]
mod store_digest;

#[cfg(any(feature = "native-engine-abi", test))]
mod full_build_driver;
#[cfg(feature = "native-engine-abi")]
mod source_publish;
#[cfg(any(feature = "native-engine-abi", test))]
mod stage_publish;

#[cfg(feature = "c-backend")]
mod python_imports;

#[cfg(feature = "c-backend")]
pub use python_imports::register as mori_register_python_imports;

#[cfg(all(
    feature = "native-engine-abi",
    any(not(test), feature = "c-backend")
))]
mod hook_augment;
