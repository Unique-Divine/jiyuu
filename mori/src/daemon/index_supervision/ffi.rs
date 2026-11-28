use super::*;

use std::{
    alloc::{Layout, alloc},
    ffi::{c_char, c_int, c_void},
    ptr,
};

type FenceCreate = unsafe extern "C" fn(*const c_char, *mut c_int) -> c_int;
type Cancel = unsafe extern "C" fn(*mut c_void) -> bool;

fn fence_operation(
    path: *const c_char,
    create: FenceCreate,
    error: &mut c_int,
) -> Option<impl FnOnce() -> FenceClaim> {
    // C supplies either NULL or a nonempty, live path in its native encoding.
    (!path.is_null()).then_some(move || match unsafe { create(path, error) } {
        0 => FenceClaim::Created,
        1 => FenceClaim::Exists,
        _ => FenceClaim::Failed,
    })
}

#[unsafe(no_mangle)]
extern "C" fn mori_index_supervision_new() -> *mut Supervision {
    // Match the supervisor's recoverable allocation failure on worker start.
    let state =
        unsafe { alloc(Layout::new::<Supervision>()).cast::<Supervision>() };
    if !state.is_null() {
        unsafe { state.write(Supervision::default()) };
    }
    state
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_index_supervision_free(state: *mut Supervision) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state)) };
    }
}

#[unsafe(no_mangle)]
extern "C" fn mori_index_worker_publication(
    already_claimed: bool,
    path: *const c_char,
    create: FenceCreate,
) -> c_int {
    worker_publication(already_claimed, fence_operation(path, create, &mut 0))
        as c_int
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_index_supervision_checks_limits(
    state: *const Supervision,
) -> bool {
    unsafe { &*state }.checks_limits()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_index_supervision_limit(
    state: *const Supervision,
    fence_path: *const c_char,
    create: FenceCreate,
    error_out: *mut c_int,
    cancel: Cancel,
    context: *mut c_void,
) -> c_int {
    let mut error = 0;
    let action = unsafe { &*state }
        .limit(fence_operation(fence_path, create, &mut error), || unsafe {
            cancel(context)
        });
    unsafe { ptr::write(error_out, error) };
    action as c_int
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_index_supervision_cancel(
    state: *const Supervision,
    cancel: Cancel,
    context: *mut c_void,
) -> bool {
    unsafe { &*state }.cancel(|| unsafe { cancel(context) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_index_supervision_terminal(
    state: *const Supervision,
    fence_path: *const c_char,
    create: FenceCreate,
    facts: TerminalFacts,
) -> TerminalDecision {
    unsafe { &*state }
        .terminal(facts, fence_operation(fence_path, create, &mut 0))
}

#[unsafe(no_mangle)]
extern "C" fn mori_index_supervision_response_disposition(
    status: c_int,
) -> c_int {
    response_disposition(status) as c_int
}
