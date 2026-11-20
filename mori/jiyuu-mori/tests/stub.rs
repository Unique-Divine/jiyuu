//! Without feature `c-backend`, `mori` links no C and says so.
#![cfg(not(feature = "c-backend"))]

use std::process::Command;

#[test]
fn stub_exits_2_with_rebuild_hint() {
    let out = Command::new(env!("CARGO_BIN_EXE_mori"))
        .arg("--version")
        .output()
        .expect("run mori");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("built without feature `c-backend`"),
        "{stderr}"
    );
    assert!(out.stdout.is_empty());
}
