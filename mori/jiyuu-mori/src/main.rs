//! `mori`: the Mori command. Rust owns the process entry point; the whole C
//! program (`codebase-memory-mcp`) runs inside through module [`ffi`], linked
//! as `libmori-c` when feature `c-backend` is on. See Strategy A in the Rust
//! port design.

mod argv;
#[cfg(feature = "c-backend")]
mod ffi;

fn main() {
    let args = argv::c_args(std::env::args_os());
    std::process::exit(run(args));
}

/// Hands argv to the C program and returns its exit code. Worker and daemon
/// re-execs (`cli --index-worker ...`, the hidden daemon argument) pass
/// through unchanged, because the C side classifies argv itself.
#[cfg(feature = "c-backend")]
fn run(args: Vec<std::ffi::CString>) -> i32 {
    ffi::run_c_main(args)
}

#[cfg(not(feature = "c-backend"))]
fn run(_args: Vec<std::ffi::CString>) -> i32 {
    eprintln!(
        "mori: built without feature `c-backend`, so the C program is not \
         linked; rebuild with `cargo build --features c-backend`"
    );
    2
}
