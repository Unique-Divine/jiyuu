//! Rust implementations and registration for the C graph engine.

pub mod ipc;

#[cfg(any(feature = "native-engine-abi", test))]
mod complexity;
#[cfg(feature = "native-engine-abi")]
mod doc_file_links;
#[cfg(any(feature = "native-engine-abi", test))]
mod explicit_override;
#[cfg(feature = "native-engine-abi")]
mod pdf_doc_links;
#[cfg(any(feature = "native-engine-abi", test))]
mod similarity;

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

#[cfg(all(feature = "c-backend", unix))]
#[path = "discovery_spike/native.rs"]
mod discover_walk;

#[cfg(all(feature = "c-backend", not(test), unix))]
pub use discover_walk::register as mori_register_discover_walk;

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
mod config_text_edit;
#[cfg(feature = "native-engine-abi")]
mod fts_writer;

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
