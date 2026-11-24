//! Schedule the full build while C owns the graph, passes, and stage.

use std::ffi::c_int;

#[repr(C)]
struct FullBuild {
    _opaque: [u8; 0],
}

type StatusOp = unsafe extern "C" fn(*mut FullBuild) -> c_int;

#[repr(C)]
struct Ops {
    structure: StatusOp,
    workers: StatusOp,
    extract: unsafe extern "C" fn(*mut FullBuild, c_int, bool) -> c_int,
    tests: StatusOp,
    history: StatusOp,
    predump: unsafe extern "C" fn(*mut FullBuild),
    fence: StatusOp,
    stage_write: StatusOp,
}

fn status(rc: c_int) -> Result<(), c_int> {
    if rc == 0 { Ok(()) } else { Err(rc) }
}

unsafe extern "C" fn drive(
    ctx: *mut FullBuild,
    ops: *const Ops,
    file_count: c_int,
    parallel_threshold: c_int,
) -> c_int {
    // SAFETY: C lends both for this synchronous call. The table's operations
    // accept the same context and retain ownership of all native state.
    let result = unsafe {
        let ops = &*ops;
        (|| {
            status((ops.structure)(ctx))?;
            let workers = (ops.workers)(ctx);
            status((ops.extract)(
                ctx,
                workers,
                workers > 1 && file_count > parallel_threshold,
            ))?;
            status((ops.tests)(ctx))?;
            status((ops.history)(ctx))?;
            (ops.predump)(ctx);
            status((ops.fence)(ctx))?;
            status((ops.stage_write)(ctx))
        })()
    };
    result.err().unwrap_or(0)
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_pipeline_set_full_build_driver(
        driver: unsafe extern "C" fn(
            *mut FullBuild,
            *const Ops,
            c_int,
            c_int,
        ) -> c_int,
    );
}

#[cfg(not(test))]
pub(crate) unsafe fn register() {
    if std::env::var_os("MORI_IMPL_FULL_BUILD_DRIVER")
        .is_none_or(|value| value == "rust")
    {
        // SAFETY: startup registration precedes all pipeline workers, and
        // the callback has process lifetime.
        unsafe { cbm_pipeline_set_full_build_driver(drive) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Trace {
        steps: Vec<&'static str>,
        workers: c_int,
        fail_at: &'static str,
    }

    fn step(ctx: *mut FullBuild, name: &'static str) -> c_int {
        // SAFETY: each mock operation receives the same live Trace.
        let trace = unsafe { &mut *ctx.cast::<Trace>() };
        trace.steps.push(name);
        if trace.fail_at == name { -2 } else { 0 }
    }

    unsafe extern "C" fn structure(ctx: *mut FullBuild) -> c_int {
        step(ctx, "structure")
    }
    unsafe extern "C" fn workers(ctx: *mut FullBuild) -> c_int {
        step(ctx, "workers");
        // SAFETY: the caller lent this Trace for the synchronous driver call.
        unsafe { (*ctx.cast::<Trace>()).workers }
    }
    unsafe extern "C" fn extract(
        ctx: *mut FullBuild,
        _: c_int,
        parallel: bool,
    ) -> c_int {
        step(ctx, if parallel { "parallel" } else { "sequential" })
    }
    unsafe extern "C" fn tests(ctx: *mut FullBuild) -> c_int {
        step(ctx, "tests")
    }
    unsafe extern "C" fn history(ctx: *mut FullBuild) -> c_int {
        step(ctx, "history")
    }
    unsafe extern "C" fn predump(ctx: *mut FullBuild) {
        step(ctx, "predump");
    }
    unsafe extern "C" fn fence(ctx: *mut FullBuild) -> c_int {
        step(ctx, "fence")
    }
    unsafe extern "C" fn stage_write(ctx: *mut FullBuild) -> c_int {
        step(ctx, "stage_write")
    }

    #[test]
    fn orders_full_build_operations_and_stops_at_failures() {
        let ops = Ops {
            structure,
            workers,
            extract,
            tests,
            history,
            predump,
            fence,
            stage_write,
        };
        for (files, workers, extraction) in [
            (50, 2, "sequential"),
            (51, 2, "parallel"),
            (51, 1, "sequential"),
        ] {
            let expected = [
                "structure",
                "workers",
                extraction,
                "tests",
                "history",
                "predump",
                "fence",
                "stage_write",
            ];
            for fail_at in [
                "",
                "structure",
                extraction,
                "tests",
                "history",
                "fence",
                "stage_write",
            ] {
                let mut trace = Trace {
                    steps: Vec::new(),
                    workers,
                    fail_at,
                };
                // SAFETY: the mock callbacks borrow Trace for the call.
                let rc =
                    unsafe { drive((&raw mut trace).cast(), &ops, files, 50) };
                if let Some(failure) =
                    expected.iter().position(|&op| op == fail_at)
                {
                    assert_eq!(rc, -2);
                    assert_eq!(trace.steps, expected[..=failure]);
                } else {
                    assert_eq!(rc, 0);
                    assert_eq!(trace.steps, expected);
                }
            }
        }
    }
}
