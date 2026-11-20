//! `mori`: the Mori command. With feature `c-backend`, the whole C program
//! (`codebase-memory-mcp`) is linked in as `libmori-c` and module [`ffi`]
//! exports the process entry point. See Strategy A in the Rust port design.

// With the C program linked, `ffi::main` is the C `main` symbol: the C
// runtime calls it with the kernel's argc and argv, and Rust's own start-up
// (`lang_start`: signal dispositions, fd 0 to 2 checks, stack-overflow
// handlers) never runs, so the process starts as `codebase-memory-mcp` does.
// A unit-test build keeps the test harness's main instead.
#![cfg_attr(all(feature = "c-backend", not(test)), no_main)]

#[cfg(all(feature = "c-backend", not(test)))]
mod ffi;

#[cfg(not(feature = "c-backend"))]
fn main() {
    eprintln!(
        "mori: built without feature `c-backend`, so the C program is not \
         linked; rebuild with `cargo build --features c-backend`"
    );
    std::process::exit(2);
}
