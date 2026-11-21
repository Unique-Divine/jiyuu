//! `mori`: the Mori command. With feature `c-backend`, the whole C program
//! (`codebase-memory-mcp`) is linked in as `libmori-c` and module [`ffi`]
//! exports the process entry point. Rust handles the developer export before
//! dispatching the other commands to the C program.

// With the C program linked, `ffi::main` is the C `main` symbol: the C
// runtime calls it with the kernel's argc and argv, and Rust's own start-up
// (`lang_start`: signal dispositions, fd 0 to 2 checks, stack-overflow
// handlers) never runs, so the process starts as `codebase-memory-mcp` does.
// A unit-test build keeps the test harness's main instead.
#![cfg_attr(all(feature = "c-backend", not(test)), no_main)]

#[cfg(all(feature = "c-backend", not(test)))]
mod export;
#[cfg(all(feature = "c-backend", not(test)))]
mod ffi;
#[cfg(all(feature = "c-backend", not(test)))]
mod store;
#[cfg(all(feature = "c-backend", not(test)))]
mod tools;

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

#[cfg(any(feature = "c-backend", test))]
mod gqlgen;
#[cfg(any(feature = "c-backend", test))]
mod link;

#[cfg(any(feature = "c-backend", test))]
mod stdio;

#[cfg(any(feature = "c-backend", test))]
mod graphql_link;

#[cfg(any(feature = "c-backend", test))]
mod query_cli;

#[cfg(any(feature = "c-backend", test))]
mod initialize;
#[cfg(any(feature = "c-backend", test))]
mod tool_list;

#[cfg(not(feature = "c-backend"))]
fn main() {
    eprintln!(
        "mori: built without feature `c-backend`, so the C program is not \
         linked; rebuild with `cargo build --features c-backend`"
    );
    std::process::exit(2);
}
