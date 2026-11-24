//! `mori`: the Mori command. With feature `c-backend`, the whole C program
//! (`codebase-memory-mcp`) is linked in as `libmori-c` and module [`ffi`]
//! exports the process entry point. Rust handles the developer export and
//! standalone UI before dispatching the remaining commands to the C program.

// With the C program linked, `ffi::main` is the C `main` symbol: the C
// runtime calls it with the kernel's argc and argv, and Rust's own start-up
// (`lang_start`: signal dispositions, fd 0 to 2 checks, stack-overflow
// handlers) never runs, so the process starts as `codebase-memory-mcp` does.
// A unit-test build keeps the test harness's main instead.
#![cfg_attr(all(feature = "c-backend", not(test)), no_main)]

#[cfg(all(feature = "c-backend", not(test)))]
mod cypher_ffi;
#[cfg(all(feature = "c-backend", not(test)))]
mod cypher_linear;
#[cfg(all(feature = "c-backend", not(test)))]
mod export;
#[cfg(all(feature = "c-backend", not(test)))]
mod ffi;
#[cfg(all(feature = "c-backend", not(test)))]
mod store;
#[cfg(all(feature = "c-backend", not(test)))]
mod tools;
#[cfg(all(feature = "c-backend", not(test)))]
mod ui;

#[cfg(feature = "c-backend")]
mod search;

#[cfg(feature = "c-backend")]
mod path_verdict;

#[cfg(feature = "c-backend")]
mod trace_predecessor;

#[cfg(all(feature = "c-backend", not(test)))]
mod index_manifest;

#[cfg(any(feature = "c-backend", test))]
mod index_route;

#[cfg(any(feature = "c-backend", test))]
mod full_build_driver;

#[cfg(any(feature = "c-backend", test))]
mod repair_recovery;

#[cfg(all(feature = "c-backend", not(test)))]
mod named_watch;

#[cfg(all(feature = "c-backend", not(test)))]
mod snippet_window;

#[cfg(any(feature = "c-backend", test))]
mod stdio;

#[cfg(any(feature = "c-backend", test))]
mod query_cli;

#[cfg(any(feature = "c-backend", test))]
mod cli_output;

#[cfg(any(feature = "c-backend", test))]
mod initialize;

#[cfg(any(feature = "c-backend", test))]
mod ingest_trace;

#[cfg(any(feature = "c-backend", test))]
mod mcp_dispatch;
#[cfg(any(feature = "c-backend", test))]
mod project_page;
#[cfg(all(feature = "c-backend", not(test)))]
mod project_response;
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
