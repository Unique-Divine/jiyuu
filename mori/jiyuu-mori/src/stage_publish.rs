//! Write and install the private database shared by full and closure indexing.

use std::ffi::{c_char, c_int};

#[repr(C)]
struct StageWrite {
    _opaque: [u8; 0],
}

type WriteOp = unsafe extern "C" fn(*mut StageWrite) -> bool;

#[repr(C)]
struct WriteOps {
    hashes: WriteOp,
    surfaces: WriteOp,
    doc_candidates: WriteOp,
    format: WriteOp,
    coverage: WriteOp,
    doc_links: WriteOp,
    fts: unsafe extern "C" fn(*mut StageWrite, bool) -> bool,
    advance: WriteOp,
    integrity: WriteOp,
    seal: WriteOp,
    mark: unsafe extern "C" fn(*mut StageWrite, *const c_char),
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_stage_write(
    run: *mut StageWrite,
    ops: &WriteOps,
    surfaces_in_place: bool,
    doc_candidates_in_place: bool,
    full_build: bool,
) -> bool {
    // SAFETY: C lends the stage and its matching callbacks for this call.
    unsafe {
        let mut ok = (ops.hashes)(run)
            && (surfaces_in_place || (ops.surfaces)(run))
            && (doc_candidates_in_place || (ops.doc_candidates)(run))
            && (ops.format)(run);
        (ops.mark)(run, c"writes".as_ptr());
        // Preserve the attempted coverage write even if an earlier write failed.
        ok = (ops.coverage)(run) && ok;
        (ops.mark)(run, c"coverage_replace".as_ptr());
        ok = ok && (ops.doc_links)(run);
        (ops.mark)(run, c"doc_links".as_ptr());
        ok = ok && (ops.fts)(run, full_build);
        (ops.mark)(run, c"fts".as_ptr());

        ok = ok && (ops.advance)(run);
        (ops.mark)(run, c"generation".as_ptr());
        ok = ok && (ops.integrity)(run);
        (ops.mark)(run, c"integrity".as_ptr());
        ok = ok && (ops.seal)(run);
        (ops.mark)(run, c"seal".as_ptr());
        ok
    }
}

#[repr(C)]
struct StageFinalize {
    _opaque: [u8; 0],
}

type Check = unsafe extern "C" fn(*mut StageFinalize) -> bool;
type Cleanup = unsafe extern "C" fn(*mut StageFinalize);

#[repr(C)]
struct FinalizeOps {
    abort_preserve_db: c_int,
    persist_failed: c_int,
    remove_sidecars: Check,
    cancelled: Check,
    prepare: Check,
    rollback: Check,
    rename: Check,
    release: Cleanup,
    discard: Cleanup,
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_stage_finalize(
    run: *mut StageFinalize,
    ops: &FinalizeOps,
) -> c_int {
    // SAFETY: C lends the closed stage and its matching callbacks. Prepare
    // owns recovery from its own errors. After it succeeds the driver owns
    // rollback, and each exit either discards or releases the stage.
    unsafe {
        let status = if !(ops.remove_sidecars)(run) {
            ops.persist_failed
        } else if (ops.cancelled)(run) {
            ops.abort_preserve_db
        } else if !(ops.prepare)(run) {
            ops.persist_failed
        } else if (ops.cancelled)(run) {
            if (ops.rollback)(run) {
                ops.abort_preserve_db
            } else {
                ops.persist_failed
            }
        } else if (ops.rename)(run) {
            (ops.release)(run);
            return 0;
        } else {
            (ops.rollback)(run);
            ops.persist_failed
        };
        (ops.discard)(run);
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct WriteTrace {
        calls: Vec<&'static str>,
        fail_at: &'static str,
    }

    macro_rules! write_actions {
        ($($action:ident),+ $(,)?) => {
            $(unsafe extern "C" fn $action(run: *mut StageWrite) -> bool {
                // SAFETY: the test lends this live trace for the driver call.
                let trace = unsafe { &mut *run.cast::<WriteTrace>() };
                trace.calls.push(stringify!($action));
                trace.fail_at != stringify!($action)
            })+
        }
    }

    write_actions!(
        hashes,
        surfaces,
        doc_candidates,
        format,
        coverage,
        doc_links,
        fts_full,
        fts_closure,
        advance,
        integrity,
        seal,
    );

    unsafe extern "C" fn fts(run: *mut StageWrite, full_build: bool) -> bool {
        // SAFETY: both callbacks borrow the same live trace as the caller.
        unsafe {
            if full_build {
                fts_full(run)
            } else {
                fts_closure(run)
            }
        }
    }

    unsafe extern "C" fn ignore_mark(_: *mut StageWrite, _: *const c_char) {}

    const WRITE_OPS: WriteOps = WriteOps {
        hashes,
        surfaces,
        doc_candidates,
        format,
        coverage,
        doc_links,
        fts,
        advance,
        integrity,
        seal,
        mark: ignore_mark,
    };

    #[test]
    fn writes_full_and_closure_metadata_before_sealing() {
        for (surfaces_in_place, doc_candidates_in_place, full_build, expected) in [
            (
                false,
                false,
                true,
                &[
                    "hashes",
                    "surfaces",
                    "doc_candidates",
                    "format",
                    "coverage",
                    "doc_links",
                    "fts_full",
                    "advance",
                    "integrity",
                    "seal",
                ][..],
            ),
            (
                true,
                true,
                false,
                &[
                    "hashes",
                    "format",
                    "coverage",
                    "doc_links",
                    "fts_closure",
                    "advance",
                    "integrity",
                    "seal",
                ],
            ),
            (
                true,
                false,
                false,
                &[
                    "hashes",
                    "doc_candidates",
                    "format",
                    "coverage",
                    "doc_links",
                    "fts_closure",
                    "advance",
                    "integrity",
                    "seal",
                ],
            ),
        ] {
            let mut trace = WriteTrace {
                calls: Vec::new(),
                fail_at: "",
            };
            // SAFETY: the driver and mocks borrow the live trace exclusively.
            assert!(unsafe {
                cbm_rust_stage_write(
                    (&raw mut trace).cast(),
                    &WRITE_OPS,
                    surfaces_in_place,
                    doc_candidates_in_place,
                    full_build,
                )
            });
            assert_eq!(trace.calls, expected);
        }
    }

    #[test]
    fn any_write_failure_prevents_success() {
        let cases: &[(&str, &[&str])] = &[
            ("hashes", &["hashes", "coverage"]),
            (
                "coverage",
                &["hashes", "surfaces", "doc_candidates", "format", "coverage"],
            ),
            (
                "fts_full",
                &[
                    "hashes",
                    "surfaces",
                    "doc_candidates",
                    "format",
                    "coverage",
                    "doc_links",
                    "fts_full",
                ],
            ),
        ];
        for (fail_at, expected) in cases {
            let mut trace = WriteTrace {
                calls: Vec::new(),
                fail_at,
            };
            // SAFETY: the driver and mocks borrow the live trace exclusively.
            assert!(!unsafe {
                cbm_rust_stage_write(
                    (&raw mut trace).cast(),
                    &WRITE_OPS,
                    false,
                    false,
                    true,
                )
            });
            assert_eq!(&trace.calls, expected);
        }
        for (fail_at, last_call) in [
            ("surfaces", "coverage"),
            ("doc_candidates", "coverage"),
            ("format", "coverage"),
            ("doc_links", "doc_links"),
            ("advance", "advance"),
            ("integrity", "integrity"),
            ("seal", "seal"),
        ] {
            let mut trace = WriteTrace {
                calls: Vec::new(),
                fail_at,
            };
            // SAFETY: the driver and mocks borrow the live trace exclusively.
            assert!(!unsafe {
                cbm_rust_stage_write(
                    (&raw mut trace).cast(),
                    &WRITE_OPS,
                    false,
                    false,
                    true,
                )
            });
            assert_eq!(trace.calls.last(), Some(&last_call));
        }
    }

    #[derive(Default)]
    struct FinalizeTrace {
        calls: Vec<&'static str>,
        cancel_at: Option<&'static str>,
        fail_at: &'static str,
        rollback_fails: bool,
        prepared: bool,
    }

    fn finalize_step(run: *mut StageFinalize, action: &'static str) -> bool {
        // SAFETY: the test lends this live trace for the driver call.
        let trace = unsafe { &mut *run.cast::<FinalizeTrace>() };
        trace.calls.push(action);
        trace.fail_at != action
    }

    unsafe extern "C" fn remove_sidecars(run: *mut StageFinalize) -> bool {
        finalize_step(run, "sidecars")
    }

    unsafe extern "C" fn cancelled(run: *mut StageFinalize) -> bool {
        // SAFETY: the test lends this live trace for the driver call.
        let trace = unsafe { &mut *run.cast::<FinalizeTrace>() };
        let step = if trace.prepared {
            "late_cancel"
        } else {
            "early_cancel"
        };
        trace.calls.push(step);
        trace.cancel_at == Some(step)
    }

    unsafe extern "C" fn prepare(run: *mut StageFinalize) -> bool {
        if !finalize_step(run, "prepare") {
            return false;
        }
        // SAFETY: the test lends this live trace for the driver call.
        unsafe { (*run.cast::<FinalizeTrace>()).prepared = true };
        true
    }

    unsafe extern "C" fn rollback(run: *mut StageFinalize) -> bool {
        finalize_step(run, "rollback");
        // SAFETY: the test lends this live trace for the driver call.
        unsafe { !(*run.cast::<FinalizeTrace>()).rollback_fails }
    }

    unsafe extern "C" fn rename(run: *mut StageFinalize) -> bool {
        finalize_step(run, "rename")
    }

    unsafe extern "C" fn release(run: *mut StageFinalize) {
        finalize_step(run, "release");
    }

    unsafe extern "C" fn discard(run: *mut StageFinalize) {
        finalize_step(run, "discard");
    }

    const ABORT: c_int = -7;
    const PERSIST: c_int = -8;
    const FINALIZE_OPS: FinalizeOps = FinalizeOps {
        abort_preserve_db: ABORT,
        persist_failed: PERSIST,
        remove_sidecars,
        cancelled,
        prepare,
        rollback,
        rename,
        release,
        discard,
    };

    #[test]
    fn closure_refresh_runs_only_after_metadata_succeeds() {
        for (fail_at, last_call) in
            [("doc_links", "doc_links"), ("fts_closure", "fts_closure")]
        {
            let mut trace = WriteTrace {
                calls: Vec::new(),
                fail_at,
            };
            // SAFETY: the driver and mocks borrow the live trace exclusively.
            assert!(!unsafe {
                cbm_rust_stage_write(
                    (&raw mut trace).cast(),
                    &WRITE_OPS,
                    true,
                    true,
                    false,
                )
            });
            assert_eq!(trace.calls.last(), Some(&last_call));
        }
    }

    #[test]
    fn finalization_rolls_back_only_after_destination_preparation() {
        let cases: &[(_, _, _, _, &[&str])] = &[
            ("sidecars", None, false, PERSIST, &["sidecars", "discard"]),
            (
                "",
                Some("early_cancel"),
                false,
                ABORT,
                &["sidecars", "early_cancel", "discard"],
            ),
            (
                "prepare",
                None,
                false,
                PERSIST,
                &["sidecars", "early_cancel", "prepare", "discard"],
            ),
            (
                "",
                Some("late_cancel"),
                false,
                ABORT,
                &[
                    "sidecars",
                    "early_cancel",
                    "prepare",
                    "late_cancel",
                    "rollback",
                    "discard",
                ],
            ),
            (
                "",
                Some("late_cancel"),
                true,
                PERSIST,
                &[
                    "sidecars",
                    "early_cancel",
                    "prepare",
                    "late_cancel",
                    "rollback",
                    "discard",
                ],
            ),
            (
                "rename",
                None,
                false,
                PERSIST,
                &[
                    "sidecars",
                    "early_cancel",
                    "prepare",
                    "late_cancel",
                    "rename",
                    "rollback",
                    "discard",
                ],
            ),
            (
                "rename",
                None,
                true,
                PERSIST,
                &[
                    "sidecars",
                    "early_cancel",
                    "prepare",
                    "late_cancel",
                    "rename",
                    "rollback",
                    "discard",
                ],
            ),
            (
                "",
                None,
                false,
                0,
                &[
                    "sidecars",
                    "early_cancel",
                    "prepare",
                    "late_cancel",
                    "rename",
                    "release",
                ],
            ),
        ];
        for &(fail_at, cancel_at, rollback_fails, status, calls) in cases {
            let mut trace = FinalizeTrace {
                fail_at,
                cancel_at,
                rollback_fails,
                ..Default::default()
            };
            // SAFETY: the driver and mocks borrow the live trace exclusively.
            assert_eq!(
                unsafe {
                    cbm_rust_stage_finalize(
                        (&raw mut trace).cast(),
                        &FINALIZE_OPS,
                    )
                },
                status
            );
            assert_eq!(trace.calls, calls);
        }
    }
}
