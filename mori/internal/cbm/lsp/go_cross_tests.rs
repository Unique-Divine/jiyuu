use std::ffi::CString;

use super::resolve;

#[test]
fn exact_target_precedes_import_translation_and_package_lookup() {
    for (name, candidates, expected) in [
        (
            c"api.Type.Open",
            &[c"api.Type.Open", c"project.api.Type.Open"][..],
            Some(c"api.Type.Open"),
        ),
        (
            c"api.Type.Open",
            &[c"project.api.Type.Open"],
            Some(c"project.api.Type.Open"),
        ),
        (c"Open", &[c"project.cli.Open"], Some(c"project.cli.Open")),
        (c"Open", &[c"other.Open"], None),
        (c"pkg.Open", &[c"project.cli.pkg.Open"], None),
        (c".Open", &[c"project.api.Open"], None),
        (c"api.", &[c"project.api."], Some(c"project.api.")),
    ] {
        assert_eq!(
            resolve(
                name,
                Some(b"project.cli"),
                |alias| (alias == b"api").then_some(b"project.api"),
                |name| candidates.iter().copied().find(|&target| target == name),
            ),
            expected,
            "{name:?}",
        );
    }
}

#[test]
fn applies_native_byte_limits_and_preserves_non_utf8() {
    let cases = [
        (b"\xff.X".to_vec(), b"pkg\xfe".to_vec(), true),
        (
            [vec![b'a'; 255], b".X".to_vec()].concat(),
            b"p".to_vec(),
            true,
        ),
        (
            [vec![b'a'; 256], b".X".to_vec()].concat(),
            b"p".to_vec(),
            false,
        ),
        (b"a.X".to_vec(), vec![b'p'; 1021], true),
        (b"a.X".to_vec(), vec![b'p'; 1022], false),
        (b"X".to_vec(), vec![b'p'; 509], true),
        (b"X".to_vec(), vec![b'p'; 510], false),
    ];
    for (name, package, should_resolve) in cases {
        let callee = CString::new(name).unwrap();
        let suffix = callee.to_bytes().splitn(2, |&b| b == b'.').last().unwrap();
        let target =
            CString::new([package.as_slice(), b".", suffix].concat()).unwrap();
        assert_eq!(
            resolve(
                &callee,
                Some(&package),
                |_| Some(&package),
                |name| { (name == target.as_c_str()).then_some(()) }
            ),
            should_resolve.then_some(()),
            "{callee:?}, package length {}",
            package.len(),
        );
    }
}
