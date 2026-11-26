//! Applies final binary link settings owned by the executable package.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var_os("CARGO_FEATURE_C_BACKEND").is_none() {
        return;
    }

    println!("cargo:rerun-if-env-changed=MORI_C_BUILD_DIR");
    let build_dir = env::var("DEP_MORI_C_BUILD_DIR")
        .expect("native archive build directory");
    println!("cargo:rustc-env=MORI_C_BUILD_DIR={build_dir}");

    // Cargo applies binary linker arguments only to binaries in this package.
    // Rust already links with RELRO, immediate binding, and a non-executable
    // stack on Linux. Separate code segments are not the default on ARM64.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins=-Wl,-z,separate-code");
    }
}
