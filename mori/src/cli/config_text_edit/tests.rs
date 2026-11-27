use super::test_hooks::{Phase, hook};
use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

struct Fixture {
    dir: PathBuf,
    file: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        loop {
            let dir = std::env::temp_dir().join(format!(
                "mori-text-editor-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&dir) {
                Ok(()) => {
                    return Self {
                        file: dir.join("AGENTS.md"),
                        dir,
                    };
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("{error}"),
            }
        }
    }

    fn no_staged_files(&self) {
        assert!(!fs::read_dir(&self.dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".cbm-text-")
        }));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const BEGIN: &str = "<!-- codebase-memory-mcp:start -->";
const END: &str = "<!-- codebase-memory-mcp:end -->";

fn block() -> Markers<'static> {
    Markers::new(BEGIN.as_bytes(), END.as_bytes()).unwrap()
}

#[test]
fn insert_then_remove_preserves_bom_comments_and_final_newline() {
    let fixture = Fixture::new();
    for (old, eol, separator, tail) in [
        (
            "\u{feff}# User heading\n<!-- arbitrary comment -->\n",
            "\n",
            "",
            "\n",
        ),
        ("user text", "\n", "\n", ""),
        ("a\r\nβ\r\n", "\r\n", "", "\r\n"),
        ("a\r\nβ", "\r\n", "\r\n", ""),
        (BOM, "\n", "", "\n"),
        ("", "\n", "", "\n"),
    ] {
        fs::write(&fixture.file, old).unwrap();
        let expected = format!(
            "{old}{separator}{BEGIN}{eol}one{eol}{eol}two{eol}{END}{tail}"
        );
        for _ in 0..2 {
            upsert(&fixture.file, block(), b"one\n\r\ntwo", MAX_BYTES).unwrap();
            assert_eq!(fs::read(&fixture.file).unwrap(), expected.as_bytes());
        }
        assert_eq!(
            block_status(&fixture.file, block()).unwrap(),
            Ownership::Owned
        );
        remove_block(&fixture.file, block()).unwrap();
        assert_eq!(fs::read(&fixture.file).unwrap(), old.as_bytes());
    }
    fixture.no_staged_files();
}

#[test]
fn replace_preserves_mixed_line_endings_outside_owned_block() {
    let fixture = Fixture::new();
    let original = format!("head\n{BEGIN}\r\nold\r\n{END}\r\ntail\n");
    let expected = format!("head\n{BEGIN}\r\nnew\r\n{END}\r\ntail\n");
    fs::write(&fixture.file, original).unwrap();
    upsert(&fixture.file, block(), b"new", expected.len()).unwrap();
    assert_eq!(fs::read(&fixture.file).unwrap(), expected.as_bytes());
    assert!(upsert(&fixture.file, block(), b"new", expected.len() - 1).is_err());
    assert_eq!(fs::read(&fixture.file).unwrap(), expected.as_bytes());
    remove_block(&fixture.file, block()).unwrap();
    assert_eq!(fs::read(&fixture.file).unwrap(), b"head\ntail\n");
}

#[test]
fn ambiguous_markers_and_invalid_text_leave_every_byte_in_place() {
    let fixture = Fixture::new();
    for original in [
        format!("{BEGIN}\n"),
        format!("{END}\n"),
        format!("{END}\n{BEGIN}"),
        format!("{BEGIN}\n{BEGIN}\n{END}\n"),
        format!("prefix {BEGIN}\n{END}\n"),
        format!("{BEGIN}\ninline {END} text\n{END}\n"),
    ] {
        fs::write(&fixture.file, &original).unwrap();
        assert!(upsert(&fixture.file, block(), b"new", MAX_BYTES).is_err());
        assert!(remove_block(&fixture.file, block()).is_err());
        assert!(block_status(&fixture.file, block()).is_err());
        assert_eq!(fs::read(&fixture.file).unwrap(), original.as_bytes());
    }
    for original in [
        b"a\xc0\xaf".as_slice(),
        b"a\xc2\x85",
        b"a\0",
        b"a\r",
        b"a\xef\xbb\xbf",
    ] {
        fs::write(&fixture.file, original).unwrap();
        assert!(
            write_document(&fixture.file, b"new", Write::Overwrite).is_err()
        );
        assert_eq!(fs::read(&fixture.file).unwrap(), original);
    }
    assert!(Markers::new(b"marker", b"longer marker").is_err());
    assert!(Markers::new(b"begin\n", b"end").is_err());
    fs::write(&fixture.file, b"keep").unwrap();
    assert!(
        upsert(&fixture.file, block(), BEGIN.as_bytes(), MAX_BYTES).is_err()
    );
    assert_eq!(fs::read(&fixture.file).unwrap(), b"keep");
    fixture.no_staged_files();
}

#[test]
fn probes_preserve_foreign_content_and_missing_documents() {
    let fixture = Fixture::new();
    assert_eq!(
        block_status(&fixture.file, block()).unwrap(),
        Ownership::Owned
    );
    assert_eq!(
        document_status(&fixture.file, b"current", &[]).unwrap(),
        Ownership::Owned
    );
    assert!(!fixture.file.exists());
    fs::write(&fixture.file, "\u{feff}\t \r\n").unwrap();
    assert_eq!(
        block_status(&fixture.file, block()).unwrap(),
        Ownership::Owned
    );
    fs::write(&fixture.file, "export default {}").unwrap();
    assert_eq!(
        block_status(&fixture.file, block()).unwrap(),
        Ownership::Foreign
    );
    assert_eq!(
        document_status(&fixture.file, b"current", &[]).unwrap(),
        Ownership::Foreign
    );
    assert_eq!(fs::read(&fixture.file).unwrap(), b"export default {}");
}

#[test]
fn conditional_create_and_exact_migration_use_byte_snapshots() {
    let fixture = Fixture::new();
    let path = &fixture.file;
    let current = b"name: mori\ntier: verify\n";
    let released: &[&[u8]] = &[b"name: mori\n", b"name: mori\r\n"];
    write_document(path, b"", Write::IfUnchanged(None)).unwrap();
    assert!(!path.exists());
    write_document(path, released[1], Write::IfUnchanged(None)).unwrap();
    assert!(write_document(path, current, Write::IfUnchanged(None)).is_err());
    assert!(
        write_document(path, current, Write::IfUnchanged(Some(released[0])))
            .is_err()
    );
    assert!(write_document(path, current, Write::Create).is_err());
    write_document(path, released[1], Write::Ensure).unwrap();
    assert!(write_document(path, current, Write::Ensure).is_err());
    assert_eq!(
        document_status(path, current, released).unwrap(),
        Ownership::Owned
    );
    assert_eq!(
        migrate(path, current, released, None).unwrap(),
        Ownership::Owned
    );
    assert_eq!(fs::read(path).unwrap(), current);
    fs::write(path, b"").unwrap();
    assert!(write_document(path, current, Write::IfUnchanged(None)).is_err());
    write_document(path, b"foreign", Write::IfUnchanged(Some(b""))).unwrap();
    assert_eq!(
        migrate(path, current, released, None).unwrap(),
        Ownership::Foreign
    );
    assert_eq!(
        remove_document(path, current, released).unwrap(),
        Ownership::Foreign
    );
    assert_eq!(fs::read(path).unwrap(), b"foreign");
    fs::write(path, released[0]).unwrap();
    assert_eq!(
        remove_document(path, current, released).unwrap(),
        Ownership::Owned
    );
    assert!(!path.exists());
    assert_eq!(
        remove_document(path, current, released).unwrap(),
        Ownership::Owned
    );
    write_document(path, current, Write::Create).unwrap();
    assert_eq!(fs::read(path).unwrap(), current);
}

#[test]
fn oversized_and_non_regular_documents_are_refused() {
    let fixture = Fixture::new();
    fs::File::create(&fixture.file)
        .unwrap()
        .set_len(MAX_BYTES as u64 + 1)
        .unwrap();
    assert!(write_document(&fixture.file, b"valid", Write::Overwrite).is_err());
    assert_eq!(
        fs::metadata(&fixture.file).unwrap().len(),
        MAX_BYTES as u64 + 1
    );
    fs::remove_file(&fixture.file).unwrap();
    fs::create_dir(&fixture.file).unwrap();
    assert!(write_document(&fixture.file, b"valid", Write::Overwrite).is_err());
}

#[test]
fn stale_destination_and_missing_target_races_preserve_the_winner() {
    for (phase, initial) in
        [(Phase::Precommit, None), (Phase::Prepublish, Some(b"old"))]
    {
        let fixture = Fixture::new();
        let backup = fixture.dir.join("previous");
        if let Some(old) = initial {
            fs::write(&fixture.file, old).unwrap();
        }
        hook(phase, move |path| {
            if initial.is_some() {
                fs::rename(path, backup).unwrap();
            }
            fs::write(path, b"winner").unwrap();
        });
        assert!(
            write_document(&fixture.file, b"new", Write::Overwrite).is_err()
        );
        assert_eq!(fs::read(&fixture.file).unwrap(), b"winner");
        fixture.no_staged_files();
    }
}

#[test]
fn exact_delete_rechecks_identity_even_for_identical_bytes() {
    let fixture = Fixture::new();
    let backup = fixture.dir.join("previous");
    fs::write(&fixture.file, b"owned").unwrap();
    hook(Phase::Prepublish, move |path| {
        fs::rename(path, backup).unwrap();
        fs::write(path, b"owned").unwrap();
    });
    assert!(remove_document(&fixture.file, b"owned", &[]).is_err());
    assert_eq!(fs::read(&fixture.file).unwrap(), b"owned");
}

#[test]
fn replaced_staging_file_is_rejected_even_for_identical_bytes() {
    let fixture = Fixture::new();
    let backup = fixture.dir.join("previous");
    let saved = backup.clone();
    hook(Phase::TempClosed, move |staged| {
        fs::rename(staged, saved).unwrap();
        fs::write(staged, b"owned").unwrap();
    });
    assert!(write_document(&fixture.file, b"owned", Write::Overwrite).is_err());
    assert!(!fixture.file.exists());
    assert_eq!(fs::read(backup).unwrap(), b"owned");
    fixture.no_staged_files();
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::{CString, c_char, c_int};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    unsafe extern "C" {
        fn cbm_config_edit_path_follow_add_root(root: *const c_char) -> c_int;
        fn cbm_config_edit_path_follow_clear();
    }

    struct FollowRoot;

    impl FollowRoot {
        fn add(path: &Path) -> Self {
            let root = CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: root is a valid, NUL-terminated path.
            assert_eq!(
                unsafe { cbm_config_edit_path_follow_add_root(root.as_ptr()) },
                0
            );
            Self
        }
    }

    impl Drop for FollowRoot {
        fn drop(&mut self) {
            // SAFETY: clears this thread's root registration.
            unsafe { cbm_config_edit_path_follow_clear() };
        }
    }

    #[test]
    fn publication_preserves_mode_and_updates_executable_mode_atomically() {
        let fixture = Fixture::new();
        fs::write(&fixture.file, b"old").unwrap();
        fs::set_permissions(&fixture.file, fs::Permissions::from_mode(0o640))
            .unwrap();
        let initial = fs::metadata(&fixture.file).unwrap();
        write_document(&fixture.file, b"current", Write::Overwrite).unwrap();
        let preserved = fs::metadata(&fixture.file).unwrap();
        assert_eq!(
            (preserved.mode(), preserved.uid(), preserved.gid()),
            (initial.mode(), initial.uid(), initial.gid())
        );

        let path = &fixture.file;
        migrate(path, b"current", &[], Some(0o755)).unwrap();
        let installed = fs::metadata(path).unwrap();
        assert_eq!(installed.mode() & 0o777, 0o755);
        assert_ne!(installed.ino(), preserved.ino());
        migrate(path, b"current", &[], Some(0o755)).unwrap();
        assert_eq!(fs::metadata(path).unwrap().ino(), installed.ino());
        assert!(migrate(path, b"current", &[], Some(0o200)).is_err());
        assert!(migrate(path, b"current", &[], Some(0o4755)).is_err());
        assert_eq!(fs::read(path).unwrap(), b"current");
    }

    #[test]
    fn managed_edits_follow_only_opted_in_symlinks_and_exact_remove_unlinks_alias()
     {
        let fixture = Fixture::new();
        let referent = fixture.dir.join("notes");
        fs::write(&referent, b"user notes").unwrap();
        symlink("notes", &fixture.file).unwrap();
        assert!(upsert(&fixture.file, block(), b"owned", MAX_BYTES).is_err());
        let _root = FollowRoot::add(&fixture.dir);
        upsert(&fixture.file, block(), b"owned", MAX_BYTES).unwrap();
        assert_eq!(fs::read_link(&fixture.file).unwrap(), Path::new("notes"));
        assert_eq!(
            fs::read(&referent).unwrap(),
            format!("user notes\n{BEGIN}\nowned\n{END}").as_bytes()
        );
        remove_block(&fixture.file, block()).unwrap();
        assert_eq!(fs::read(&referent).unwrap(), b"user notes");
        assert_eq!(
            remove_document(&fixture.file, b"user notes", &[]).unwrap(),
            Ownership::Owned
        );
        assert!(fs::symlink_metadata(&fixture.file).is_err());
        assert_eq!(fs::read(&referent).unwrap(), b"user notes");
        fixture.no_staged_files();
    }

    #[test]
    fn swapped_symlink_does_not_publish_to_either_target() {
        let fixture = Fixture::new();
        let original = fixture.dir.join("first");
        let winner = fixture.dir.join("second");
        fs::write(&original, b"one").unwrap();
        fs::write(&winner, b"two").unwrap();
        symlink("first", &fixture.file).unwrap();
        let _root = FollowRoot::add(&fixture.dir);
        hook(Phase::Prepublish, |path| {
            fs::remove_file(path).unwrap();
            symlink("second", path).unwrap();
        });
        assert!(
            write_document(&fixture.file, b"updated", Write::Overwrite).is_err()
        );
        assert_eq!(fs::read(&original).unwrap(), b"one");
        assert_eq!(fs::read(&winner).unwrap(), b"two");
        fixture.no_staged_files();
    }

    #[test]
    fn hard_links_fifos_and_privileged_modes_are_refused() {
        let fixture = Fixture::new();
        let alias = fixture.dir.join("alias");
        fs::write(&fixture.file, b"keep").unwrap();
        fs::hard_link(&fixture.file, &alias).unwrap();
        assert!(
            write_document(&fixture.file, b"new", Write::Overwrite).is_err()
        );
        fs::remove_file(alias).unwrap();
        fs::set_permissions(&fixture.file, fs::Permissions::from_mode(0o4755))
            .unwrap();
        assert!(
            write_document(&fixture.file, b"new", Write::Overwrite).is_err()
        );
        assert_eq!(fs::read(&fixture.file).unwrap(), b"keep");
        fs::remove_file(&fixture.file).unwrap();
        let path = CString::new(fixture.file.as_os_str().as_bytes()).unwrap();
        // SAFETY: path is NUL-terminated.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(
            write_document(&fixture.file, b"new", Write::Overwrite).is_err()
        );
        fixture.no_staged_files();
    }
}
