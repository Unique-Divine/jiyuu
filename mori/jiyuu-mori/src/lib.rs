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
