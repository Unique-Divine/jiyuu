//! Rust directory iteration, selection, and admission over native inputs.

mod budget;
mod selection;

use crate::abi_rows::write_optional;
use budget::{BOUNDED_COUNT, Budget, Limits, Violation};
use selection::{Check, Decision};
use std::borrow::Cow;
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::mem::MaybeUninit;
use std::panic::catch_unwind;
use std::ptr::NonNull;

#[repr(C)]
pub(super) struct Walk {
    _opaque: [u8; 0],
}

#[repr(C)]
pub(super) struct Frame {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Selection {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Options {
    _opaque: [u8; 0],
}

#[repr(C)]
struct EntryOps {
    admit: unsafe extern "C" fn(*mut c_void, i64, *mut Violation) -> bool,
    select: unsafe extern "C" fn(*const Selection, bool) -> Decision,
}

static ENTRY_OPS: EntryOps = EntryOps { admit, select };

type BoundedWalk =
    unsafe extern "C" fn(*mut Walk, *mut Frame, *mut c_void) -> bool;

unsafe extern "C" {
    fn cbm_discover_run_bounded(
        repo: *const c_char,
        opts: *const Options,
        run: BoundedWalk,
        state: *mut c_void,
    ) -> bool;
    fn cbm_discover_walk_limits(walk: *const Walk, limits: *mut Limits);
    fn cbm_discover_walk_prepare(walk: *mut Walk, frame: *mut Frame) -> bool;
    fn cbm_discover_frame_path(frame: *const Frame) -> *const c_char;
    fn cbm_discover_walk_entry(
        walk: *mut Walk,
        frame: *const Frame,
        name: *const c_char,
        ops: *const EntryOps,
        state: *mut c_void,
    ) -> *mut Frame;
    fn cbm_discover_frame_free(frame: *mut Frame);
    fn cbm_discover_walk_stopped(walk: *mut Walk) -> bool;
    fn cbm_discover_selection_check(
        entry: *const Selection,
        check: Check,
    ) -> c_int;
    fn cbm_now_ms() -> u64;
    fn cbm_max_file_bytes() -> c_long;
    fn cbm_log(level: c_int, event: *const c_char, ...);
}

struct WalkState {
    budget: Budget,
    refusal: Option<Violation>,
}

impl WalkState {
    fn new(limits: Limits) -> Self {
        Self {
            budget: Budget::new(limits),
            refusal: None,
        }
    }
}

// cbm_discover_status_t
const ERROR: c_int = -1;
const OK: c_int = 0;
const LIMIT_EXCEEDED: c_int = 1;

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_discover_count_bounded(
    repo: *const c_char,
    opts: *const Options,
    limits: *const Limits,
    violation_out: *mut Violation,
    count_out: *mut c_int,
) -> c_int {
    // SAFETY: The C ABI shim lends initialized limits and the caller's
    // options and output slots for this synchronous call.
    unsafe {
        write_optional(count_out, -1);
        let limits = limits.read();
        if repo.is_null() || count_out.is_null() || limits.max_files < 0 {
            return ERROR;
        }
        write_optional(violation_out, Violation::default());
        let mut state = WalkState::new(limits);
        let completed = cbm_discover_run_bounded(
            repo,
            opts,
            bounded_walk,
            std::ptr::from_mut(&mut state).cast(),
        );
        let refused = state.refusal.is_some();
        if let Some(violation) = state.refusal
            && violation.resource != BOUNDED_COUNT
        {
            write_optional(violation_out, violation);
        }
        if !completed {
            return ERROR;
        }
        // Admission stops before the count can exceed max_files, a c_int.
        write_optional(count_out, state.budget.count() as c_int);
        if refused { LIMIT_EXCEEDED } else { OK }
    }
}

unsafe extern "C" fn bounded_walk(
    walk: *mut Walk,
    root: *mut Frame,
    state: *mut c_void,
) -> bool {
    const CBM_LOG_DEBUG: c_int = 0;
    // SAFETY: C lends the prepared walk and frame. State points to the
    // WalkState passed to cbm_discover_run_bounded above.
    catch_unwind(|| unsafe {
        cbm_log(
            CBM_LOG_DEBUG,
            c"discover.admission_backend".as_ptr(),
            c"implementation".as_ptr(),
            c"rust".as_ptr(),
            std::ptr::null::<c_char>(),
        );
        walk_tree(walk, root, true, &mut *state.cast::<WalkState>())
    })
    .unwrap_or(false)
}

struct Child(NonNull<Frame>);

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: walk_entry transfers ownership of each returned child.
        unsafe { cbm_discover_frame_free(self.0.as_ptr()) };
    }
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn mori_discover_walk(
    walk: *mut Walk,
    root: *mut Frame,
) -> bool {
    // SAFETY: C lends the walk state and root until the callback returns.
    // Keep the root borrowed and release all owned children before returning.
    catch_unwind(|| unsafe {
        let mut limits = MaybeUninit::uninit();
        cbm_discover_walk_limits(walk, limits.as_mut_ptr());
        walk_tree(walk, root, false, &mut WalkState::new(limits.assume_init()))
    })
    .unwrap_or(false)
}

unsafe fn walk_tree(
    walk: *mut Walk,
    root: *mut Frame,
    count_only: bool,
    state: &mut WalkState,
) -> bool {
    // SAFETY: Both entry points borrow live native walk and root state.
    unsafe {
        if state.budget.deadline_expired(|| cbm_now_ms()) {
            return false;
        }
        let mut pending = Vec::new();
        if !scan_directory(walk, root, &mut pending, count_only, state) {
            return false;
        }
        // Match the reference order. Scan each parent before visiting its
        // children in reverse discovery order. This fixes the ignore sample.
        while let Some(child) = pending.pop() {
            if state.budget.deadline_expired(|| cbm_now_ms()) {
                return false;
            }
            if state.refusal.is_some() || cbm_discover_walk_stopped(walk) {
                break;
            }
            if !scan_directory(
                walk,
                child.0.as_ptr(),
                &mut pending,
                count_only,
                state,
            ) {
                return false;
            }
        }
        true
    }
}

unsafe fn scan_directory(
    walk: *mut Walk,
    frame: *mut Frame,
    pending: &mut Vec<Child>,
    count_only: bool,
    state: &mut WalkState,
) -> bool {
    // SAFETY: walk and frame are valid for this scan. The path borrows the
    // frame. Native policy copies the name before it returns.
    unsafe {
        if !cbm_discover_walk_prepare(walk, frame) {
            return false;
        }
        let path = CStr::from_ptr(cbm_discover_frame_path(frame));
        // Bounded admission fails if a directory cannot be opened.
        let Some(mut entries) = Directory::open(path) else {
            return !count_only;
        };
        loop {
            if state.budget.deadline_expired(|| cbm_now_ms()) {
                return false;
            }
            if state.refusal.is_some() || cbm_discover_walk_stopped(walk) {
                break;
            }
            let Some(name) = entries.next_name() else {
                break;
            };
            if let Some(child) = NonNull::new(cbm_discover_walk_entry(
                walk,
                frame,
                name.as_ptr(),
                &ENTRY_OPS,
                std::ptr::from_mut(state).cast(),
            ))
            .map(Child)
            {
                if pending.try_reserve(1).is_err() {
                    return false;
                }
                pending.push(child);
            }
        }
        true
    }
}

#[cfg(unix)]
struct Directory(std::fs::ReadDir);

#[cfg(unix)]
impl Directory {
    fn open(path: &CStr) -> Option<Self> {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::path::Path;

        std::fs::read_dir(Path::new(OsStr::from_bytes(path.to_bytes())))
            .ok()
            .map(Self)
    }

    // Like readdir in the reference, an iteration error ends this directory.
    fn next_name(&mut self) -> Option<Cow<'_, CStr>> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStringExt;

        let entry = self.0.next()?.ok()?;
        Some(Cow::Owned(
            CString::new(entry.file_name().into_vec())
                .expect("directory entry names contain no NUL"),
        ))
    }
}

#[cfg(windows)]
struct Directory(NonNull<c_void>);

#[cfg(windows)]
unsafe extern "C" {
    fn cbm_opendir(path: *const c_char) -> *mut c_void;
    fn cbm_discover_dir_next(directory: *mut c_void) -> *const c_char;
    fn cbm_closedir(directory: *mut c_void);
}

#[cfg(windows)]
impl Directory {
    fn open(path: &CStr) -> Option<Self> {
        // SAFETY: cbm_opendir takes a UTF-8 C string and returns an owned
        // enumeration handle. Keep its Win32 path and encoding behavior.
        NonNull::new(unsafe { cbm_opendir(path.as_ptr()) }).map(Self)
    }

    fn next_name(&mut self) -> Option<Cow<'_, CStr>> {
        // SAFETY: The name is valid until the next read or close. The borrow
        // prevents either operation while the caller uses it.
        let name = unsafe { cbm_discover_dir_next(self.0.as_ptr()) };
        (!name.is_null()).then(|| Cow::Borrowed(unsafe { CStr::from_ptr(name) }))
    }
}

#[cfg(windows)]
impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: Only this wrapper owns the enumeration handle.
        unsafe { cbm_closedir(self.0.as_ptr()) };
    }
}

unsafe extern "C" fn select(entry: *const Selection, is_dir: bool) -> Decision {
    // SAFETY: C lends the selection and its referenced paths and metadata
    // until this callback returns.
    let check = |check| unsafe { cbm_discover_selection_check(entry, check) };
    if is_dir {
        selection::directory(check)
    } else {
        selection::file(check)
    }
}

unsafe extern "C" fn admit(
    state: *mut c_void,
    size: i64,
    violation: *mut Violation,
) -> bool {
    // SAFETY: walk_entry lends live state and provides a writable diagnostic
    // for manifest discovery. The native extraction ceiling is positive.
    let state = unsafe { &mut *state.cast::<WalkState>() };
    match state
        .budget
        .admit(size, || unsafe { cbm_max_file_bytes() as u64 })
    {
        Ok(()) => true,
        Err(reason) => {
            if violation.is_null() {
                state.refusal = Some(reason);
            } else {
                unsafe { write_optional(violation, reason) };
            }
            false
        }
    }
}
