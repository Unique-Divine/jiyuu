use std::path::PathBuf;

pub fn c_reference_bin() -> Option<PathBuf> {
    let bin = PathBuf::from(std::env::var_os("MORI_C_REFERENCE_BIN")?);
    assert!(
        bin.is_absolute() && bin.is_file(),
        "C reference must be an absolute path to a binary, got {}",
        bin.display()
    );
    Some(bin)
}
