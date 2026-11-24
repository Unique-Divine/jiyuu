use std::path::{Path, PathBuf};

pub fn c_reference_bin() -> Option<PathBuf> {
    let bin = std::env::var_os("MORI_C_REFERENCE_BIN")
        .map(PathBuf::from)
        .or_else(|| {
            (option_env!("MORI_C_REFERENCE") == Some("1")).then(|| {
                Path::new(env!("MORI_C_BUILD_DIR")).join("codebase-memory-mcp")
            })
        })?;
    assert!(
        bin.is_absolute() && bin.is_file(),
        "C reference must be an absolute path to a binary, got {}",
        bin.display()
    );
    Some(bin)
}
