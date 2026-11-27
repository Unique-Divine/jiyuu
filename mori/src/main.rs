//! `mori`: the Mori command. With feature `c-backend`, the whole C program
//! (`codebase-memory-mcp`) is linked in as `libmori-c` and module [`ffi`]
//! exports the process entry point. Rust dispatches developer export,
//! standalone UI, and top-level help and version before the C entry point.

// With the C program linked, `ffi::main` is the C `main` symbol: the C
// runtime calls it with the kernel's argc and argv, and Rust's own start-up
// (`lang_start`: signal dispositions, fd 0 to 2 checks, stack-overflow
// handlers) never runs, so the process starts as `codebase-memory-mcp` does.
// A unit-test build keeps the test harness's main instead.
#![cfg_attr(all(feature = "c-backend", not(test)), no_main)]

#[cfg(all(feature = "c-backend", not(test)))]
#[path = "cypher/cypher_ffi.rs"]
mod cypher_ffi;
#[cfg(all(feature = "c-backend", not(test)))]
#[path = "cypher/cypher_linear/mod.rs"]
mod cypher_linear;
#[cfg(all(feature = "c-backend", not(test)))]
#[path = "cli/export.rs"]
mod export;
#[cfg(all(feature = "c-backend", not(test)))]
mod ffi;
#[cfg(all(feature = "c-backend", not(test)))]
#[path = "store/store.rs"]
mod store;
#[cfg(all(feature = "c-backend", not(test)))]
#[path = "mcp/tools.rs"]
mod tools;
#[cfg(all(feature = "c-backend", not(test)))]
mod ui;

#[cfg(feature = "c-backend")]
#[path = "mcp/search.rs"]
mod search;

#[cfg(feature = "c-backend")]
#[path = "discover/path_verdict.rs"]
mod path_verdict;

#[cfg(feature = "c-backend")]
#[path = "mcp/trace_predecessor.rs"]
mod trace_predecessor;

#[cfg(any(feature = "c-backend", test))]
#[path = "mcp/index_route.rs"]
mod index_route;

#[cfg(any(feature = "c-backend", test))]
#[path = "pipeline/importance.rs"]
mod importance;

#[cfg(any(feature = "c-backend", test))]
#[path = "watcher/repair_recovery.rs"]
mod repair_recovery;

#[cfg(any(feature = "c-backend", test))]
#[path = "watcher/named_watch.rs"]
mod named_watch;

#[cfg(any(feature = "c-backend", test))]
#[path = "cli/query_cli.rs"]
mod query_cli;

#[cfg(any(feature = "c-backend", test))]
#[path = "cli/cli_output.rs"]
mod cli_output;

#[cfg(any(feature = "c-backend", test))]
#[path = "mcp/initialize.rs"]
mod initialize;

#[cfg(any(feature = "c-backend", test))]
#[path = "traces/ingest_trace.rs"]
mod ingest_trace;

#[cfg(any(feature = "c-backend", test))]
#[path = "mcp/mcp_dispatch/mod.rs"]
mod mcp_dispatch;
#[cfg(any(feature = "c-backend", test))]
#[path = "mcp/tool_list.rs"]
mod tool_list;

#[cfg(not(feature = "c-backend"))]
fn main() {
    eprintln!(
        "mori: built without feature `c-backend`, so the C program is not \
         linked; rebuild with `cargo build --features c-backend`"
    );
    std::process::exit(2);
}
