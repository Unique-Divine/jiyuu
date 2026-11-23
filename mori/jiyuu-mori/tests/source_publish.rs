//! Run the native publication hooks with each source publication backend.
#![cfg(all(feature = "c-backend", unix))]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const FAST: c_int = 2;
const ABORT_PRESERVE_DB: c_int = -2;
const PERSIST_FAILED: c_int = -4;

#[repr(C)]
struct Pipeline {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn cbm_alloc_init();
    fn cbm_pipeline_new(
        repo: *const c_char,
        db: *const c_char,
        mode: c_int,
    ) -> *mut Pipeline;
    fn cbm_pipeline_free(pipeline: *mut Pipeline);
    fn cbm_pipeline_run(pipeline: *mut Pipeline) -> c_int;
    fn cbm_pipeline_cancel(pipeline: *mut Pipeline);
    fn cbm_pipeline_set_before_publish_hook_for_tests(
        pipeline: *mut Pipeline,
        hook: unsafe extern "C" fn(*mut Pipeline, *const c_char, *mut c_void),
        context: *mut c_void,
    );
    fn cbm_pipeline_set_rename_hook_for_tests(
        pipeline: *mut Pipeline,
        hook: unsafe extern "C" fn(
            *const c_char,
            *const c_char,
            *mut c_void,
        ) -> c_int,
        context: *mut c_void,
    );
    fn cbm_pipeline_set_publish_gate(
        pipeline: *mut Pipeline,
        gate: unsafe extern "C" fn(*mut c_void) -> bool,
        context: *mut c_void,
    );
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    Cancel,
    Config,
    Lease,
    Rename,
    Sidecar,
    ReopenWal,
}

struct Hooks {
    fault: Fault,
    blocked_sidecar: PathBuf,
    config: PathBuf,
    stage: Option<PathBuf>,
    staged_db_seen: bool,
    lease_calls: usize,
    rename_calls: usize,
}

unsafe extern "C" fn before_publish(
    pipeline: *mut Pipeline,
    stage: *const c_char,
    data: *mut c_void,
) {
    // SAFETY: C lends the native stage path and the fixture's Hooks.
    let (hook, stage) =
        unsafe { (&mut *data.cast::<Hooks>(), CStr::from_ptr(stage)) };
    let stage = PathBuf::from(std::ffi::OsStr::from_bytes(stage.to_bytes()));
    hook.staged_db_seen = fs::metadata(&stage).is_ok_and(|meta| meta.len() > 0);
    if matches!(hook.fault, Fault::ReopenWal) {
        hook.staged_db_seen &= rusqlite::Connection::open(&stage)
            .and_then(|db| {
                db.query_row("PRAGMA journal_mode=WAL", [], |row| {
                    row.get::<_, String>(0)
                })
            })
            .is_ok_and(|mode| mode == "wal");
    }
    hook.stage = Some(stage);
    match hook.fault {
        // SAFETY: C owns this live pipeline until the hook returns.
        Fault::Cancel => unsafe { cbm_pipeline_cancel(pipeline) },
        Fault::Config => {
            hook.staged_db_seen &= fs::write(&hook.config, "{}").is_ok();
        }
        Fault::Sidecar => {
            hook.staged_db_seen &= fs::create_dir(&hook.blocked_sidecar).is_ok();
        }
        _ => {}
    }
}

unsafe extern "C" fn claim_lease(data: *mut c_void) -> bool {
    // SAFETY: the fixture lends Hooks for the synchronous pipeline run.
    let hook = unsafe { &mut *data.cast::<Hooks>() };
    hook.lease_calls += 1;
    !matches!(hook.fault, Fault::Lease)
}

unsafe extern "C" fn fail_rename(
    _stage: *const c_char,
    _target: *const c_char,
    data: *mut c_void,
) -> c_int {
    // SAFETY: the fixture lends Hooks for the synchronous pipeline run.
    unsafe { (*data.cast::<Hooks>()).rename_calls += 1 };
    -1
}

fn index(repo: &Path, db: &Path, fault: Fault) -> (c_int, Hooks) {
    let mut sidecar = db.as_os_str().to_os_string();
    sidecar.push("-shm");
    let mut hooks = Hooks {
        fault,
        blocked_sidecar: sidecar.into(),
        config: repo.join(".codebase-memory.json"),
        stage: None,
        staged_db_seen: false,
        lease_calls: 0,
        rename_calls: 0,
    };
    let repo = CString::new(repo.as_os_str().as_bytes()).unwrap();
    let db = CString::new(db.as_os_str().as_bytes()).unwrap();
    let context = (&raw mut hooks).cast();
    // SAFETY: C borrows valid paths and Hooks, and the fixture owns the pipeline.
    let rc = unsafe {
        let pipeline = cbm_pipeline_new(repo.as_ptr(), db.as_ptr(), FAST);
        assert!(!pipeline.is_null());
        cbm_pipeline_set_before_publish_hook_for_tests(
            pipeline,
            before_publish,
            context,
        );
        cbm_pipeline_set_publish_gate(pipeline, claim_lease, context);
        if matches!(fault, Fault::Rename) {
            cbm_pipeline_set_rename_hook_for_tests(
                pipeline,
                fail_rename,
                context,
            );
        }
        let rc = cbm_pipeline_run(pipeline);
        cbm_pipeline_free(pipeline);
        rc
    };
    if let Some(stage) = &hooks.stage {
        assert!(hooks.staged_db_seen);
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut member = stage.as_os_str().to_os_string();
            member.push(suffix);
            assert!(!Path::new(&member).exists(), "{member:?}");
        }
    }
    if matches!(fault, Fault::Sidecar) {
        fs::remove_dir(&hooks.blocked_sidecar).unwrap();
    }
    if matches!(fault, Fault::Config) {
        fs::remove_file(&hooks.config).unwrap();
    }
    (rc, hooks)
}

fn bytes_and_inode(db: &Path) -> (Vec<u8>, u64) {
    (fs::read(db).unwrap(), fs::metadata(db).unwrap().ino())
}

fn exercise(root: &Path) {
    let repo = root.join("source");
    fs::create_dir_all(&repo).unwrap();
    let db = root.join("live.db");
    fs::write(repo.join("main.py"), "def before():\n    return 1\n").unwrap();
    let (rc, first) = index(&repo, &db, Fault::None);
    assert_eq!(rc, 0);
    assert_eq!(first.lease_calls, 1);
    let original = bytes_and_inode(&db);

    let (rc, noop) = index(&repo, &db, Fault::Lease);
    assert_eq!(rc, 0);
    assert!(noop.stage.is_none());
    assert_eq!(noop.lease_calls, 0);
    assert_eq!(bytes_and_inode(&db), original);

    for file in 0..2 {
        fs::write(
            repo.join(format!("candidate{file}.py")),
            format!("def after_{file}():\n    return 2\n"),
        )
        .unwrap();
    }
    for (fault, expected, leases, renames) in [
        (Fault::Cancel, ABORT_PRESERVE_DB, 0, 0),
        (Fault::Config, ABORT_PRESERVE_DB, 0, 0),
        (Fault::Lease, ABORT_PRESERVE_DB, 1, 0),
        (Fault::Sidecar, PERSIST_FAILED, 1, 0),
        (Fault::Rename, PERSIST_FAILED, 1, 1),
    ] {
        let (rc, hook) = index(&repo, &db, fault);
        assert_eq!(rc, expected);
        assert!(hook.stage.is_some());
        assert_eq!((hook.lease_calls, hook.rename_calls), (leases, renames));
        assert_eq!(bytes_and_inode(&db), original);
    }

    let (rc, success) = index(&repo, &db, Fault::ReopenWal);
    assert_eq!(rc, 0);
    assert!(success.stage.is_some());
    assert_eq!(success.lease_calls, 1);
    assert_ne!(bytes_and_inode(&db).1, original.1);
    let store = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let added: i64 = store.query_row(
        "SELECT count(*) FROM nodes WHERE label='Function' AND name='after_0'", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(added, 1);
    let journal: String = store
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "delete");

    // An uncopyable file with no sidecars is quarantined before the failing
    // rename. Recovery must put back the same file, not recreate its bytes.
    let corrupt = root.join("corrupt.db");
    fs::write(&corrupt, "uncopyable generation").unwrap();
    let original = bytes_and_inode(&corrupt);
    let (rc, rolled_back) = index(&repo, &corrupt, Fault::Rename);
    assert_eq!(rc, PERSIST_FAILED);
    assert_eq!(rolled_back.rename_calls, 1);
    assert_eq!(bytes_and_inode(&corrupt), original);
}

#[test]
fn before_publish_preserves_serving_generation() {
    const ROOT: &str = "MORI_PUBLISH_TEST_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        // SAFETY: this separate process runs only this test and has not
        // started indexing or used SQLite.
        unsafe { cbm_alloc_init() };
        let backend = std::env::var("MORI_IMPL_SOURCE_PUBLISH").unwrap();
        if backend == "rust" {
            // SAFETY: this is the only test and indexing has not started.
            unsafe { jiyuu_mori::mori_register_source_publish() };
        }
        exercise(&PathBuf::from(root).join(backend));
        return;
    }

    let scratch = Command::new("mktemp")
        .args(["-d", "/tmp/mori-publish-XXXXXXXX"])
        .output()
        .unwrap();
    assert!(scratch.status.success(), "{scratch:?}");
    let root = PathBuf::from(std::ffi::OsStr::from_bytes(
        scratch.stdout.strip_suffix(b"\n").unwrap(),
    ));
    let cache = root.join("cache");
    fs::create_dir(&cache).unwrap();
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o700)).unwrap();
    let outputs = ["c", "rust"].map(|backend| {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "before_publish_preserves_serving_generation",
                "--nocapture",
            ])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &root)
            .env("CBM_CACHE_DIR", &cache)
            .env("CBM_WORKERS", "1")
            .env("MORI_IMPL_SOURCE_PUBLISH", backend)
            .env(ROOT, &root)
            .output()
            .unwrap();
        (backend, output)
    });
    fs::remove_dir_all(root).unwrap();
    for (backend, output) in outputs {
        let logs = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{backend}: {}\n{logs}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(
            logs.contains("msg=pipeline.source_publish implementation=rust"),
            backend == "rust",
            "{logs}"
        );
    }
}
