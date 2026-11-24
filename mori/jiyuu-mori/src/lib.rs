//! Rust GraphQL implementation and registration for the C graph engine.

#[cfg(any(feature = "graphql-callbacks", test))]
mod gqlgen;
#[cfg(any(feature = "graphql-callbacks", test))]
mod graphql_link;
#[cfg(any(feature = "graphql-callbacks", test))]
mod link;
#[cfg(any(feature = "graphql-callbacks", test))]
mod route_writer;

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
mod source_publish;

#[cfg(feature = "c-backend")]
pub use source_publish::register as mori_register_source_publish;
