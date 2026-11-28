//! Links the native archive prepared by mori-native-sys.

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

    // Keep the archive at this package's link position so its SQLite symbols
    // satisfy references before the linker considers rusqlite's bundled copy.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    emit_link_lines(&build_dir, &target_os);
}

/// Links the selected archive whole along with the libraries from
/// `LDFLAGS` in `Makefile.cbm` (`-lm -lstdc++ -lpthread -lz`).
///
/// With `-C target-feature=+crt-static` (the static Linux release, like
/// `STATIC=1` for the C binary), zlib and libstdc++ link as archives too: a
/// `dylib` library would make the linker pick the shared object and produce
/// a dynamic binary. libm and libpthread come with the static libc that
/// Rust's std links under crt-static.
fn emit_link_lines(build_dir: &str, target_os: &str) {
    let crt_static = env::var("CARGO_CFG_TARGET_FEATURE")
        .is_ok_and(|features| features.split(',').any(|f| f == "crt-static"));
    println!("cargo:rustc-link-search=native={}", build_dir);
    println!("cargo:rustc-link-lib=static:+whole-archive=mori-c");
    if crt_static && target_os == "linux" {
        // Resolve system archives at the final native link. Bundling them in
        // the Rust rlib would require teaching rustc the platform's GCC paths.
        println!("cargo:rustc-link-lib=static:-bundle=z");
        println!("cargo:rustc-link-lib=static:-bundle=stdc++");
    } else {
        println!("cargo:rustc-link-lib=dylib=z");
        println!("cargo:rustc-link-lib=dylib=m");
        println!("cargo:rustc-link-lib=dylib=pthread");
        if target_os == "macos" {
            println!("cargo:rustc-link-lib=dylib=c++");
        } else {
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
    }
    if target_os == "linux" {
        // ELF_HARDENING_FLAGS in Makefile.cbm. Rust already links with
        // `-z relro -z now` and a non-executable stack; separate-code is the
        // one it does not set (not a default on aarch64 GNU ld).
        println!("cargo:rustc-link-arg-bins=-Wl,-z,separate-code");
    }
}
