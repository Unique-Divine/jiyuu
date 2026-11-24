//! C ABI declared by cli/activation_transaction.h.

#[cfg(any(test, feature = "native-engine-test-seams"))]
use std::cell::Cell;
use std::cell::UnsafeCell;
#[cfg(any(test, feature = "native-engine-test-seams"))]
use std::ffi::c_ulong;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io;
use std::ptr;

#[cfg(any(test, feature = "native-engine-test-seams"))]
use super::platform;
use super::{Candidate, State, Status, Transaction};

#[cfg(any(test, feature = "native-engine-test-seams"))]
type Callback = unsafe extern "C" fn(*const c_char, *mut c_void);
type Validator = unsafe extern "C" fn(*const c_char, *mut c_void) -> bool;

thread_local! {
    static REFUSAL: UnsafeCell<[u8; 512]> = const { UnsafeCell::new([0; 512]) };
    #[cfg(any(test, feature = "native-engine-test-seams"))]
    static BEFORE_ABSENT: Cell<(Option<Callback>, *mut c_void)> = const {
        Cell::new((None, ptr::null_mut()))
    };
    #[cfg(any(test, feature = "native-engine-test-seams"))]
    static WRITE_FAILURE: Cell<(u32, c_int)> = const { Cell::new((0, 0)) };
    #[cfg(all(target_os = "linux", any(test, feature = "native-engine-test-seams")))]
    static HOME: std::cell::RefCell<Option<(std::ffi::CString, libc::uid_t)>> = const {
        std::cell::RefCell::new(None)
    };
}

fn clear_note() {
    REFUSAL.with(|buffer| {
        // SAFETY: each thread owns its buffer, and the C ABI treats the pointer as read-only.
        unsafe { (*buffer.get())[0] = 0 };
    });
}

pub(super) fn note(predicate: &str, detail: &str, object: Option<&CStr>) {
    REFUSAL.with(|buffer| {
        // SAFETY: this thread owns the buffer and no reference is held by the FFI getter.
        let buffer = unsafe { &mut *buffer.get() };
        if buffer[0] != 0 {
            return;
        }
        let detail = format!("{predicate} ({detail})");
        let bytes = detail.bytes().chain(object.into_iter().flat_map(|path| {
            b" at "
                .iter()
                .copied()
                .chain(path.to_bytes().iter().copied())
        }));
        let mut length = 0;
        for byte in bytes.take(buffer.len() - 1) {
            buffer[length] = byte;
            length += 1;
        }
        buffer[length] = 0;
    });
}

#[cfg(any(test, feature = "native-engine-test-seams"))]
pub(super) fn before_absent_publish(path: &CStr) {
    let (callback, context) = BEFORE_ABSENT.with(Cell::get);
    if let Some(callback) = callback {
        // SAFETY: the C caller registered a live callback and context for this thread.
        unsafe { callback(path.as_ptr(), context) };
    }
}

pub(super) fn check_write_failure() -> io::Result<()> {
    #[cfg(any(test, feature = "native-engine-test-seams"))]
    if let Some(error) = WRITE_FAILURE.with(|failure| {
        let (count, error) = failure.get();
        if count == 0 {
            return None;
        }
        failure.set((count - 1, error));
        Some(error)
    }) {
        return Err(io::Error::from_raw_os_error(error));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn home_root() -> (std::ffi::CString, libc::uid_t) {
    #[cfg(any(test, feature = "native-engine-test-seams"))]
    if let Some(home) = HOME.with(|value| value.borrow().clone()) {
        return home;
    }
    (c"/home".to_owned(), 0)
}

fn status(result: Result<(), Status>) -> c_int {
    result.err().unwrap_or(Status::Ok) as c_int
}

unsafe fn stage(
    path: *const c_char,
    out: *mut *mut Transaction,
    replace: bool,
    write: impl FnOnce(&mut Transaction) -> Result<(), Status>,
) -> c_int {
    clear_note();
    // SAFETY: out is null or a writable C output argument. path is a C string.
    let Some(out) = (unsafe { out.as_mut() }) else {
        return Status::InvalidArgument as c_int;
    };
    *out = ptr::null_mut();
    if path.is_null() {
        return Status::InvalidArgument as c_int;
    }
    let path = unsafe { CStr::from_ptr(path) };
    status((|| {
        // Allocate before staging so allocation failure cannot strand a backup.
        let layout = std::alloc::Layout::new::<Transaction>();
        // SAFETY: the layout is nonzero and suitable for Transaction.
        let allocation =
            unsafe { std::alloc::alloc(layout).cast::<Transaction>() };
        if allocation.is_null() {
            return Err(Status::NoMemory);
        }
        // SAFETY: the uninitialized allocation has the layout of MaybeUninit<Transaction>.
        let mut slot = unsafe {
            Box::from_raw(
                allocation.cast::<std::mem::MaybeUninit<Transaction>>(),
            )
        };
        let mut tx = Transaction::prepare(path, replace)?;
        write(&mut tx)?;
        slot.write(tx);
        *out = Box::into_raw(slot).cast();
        Ok(())
    })())
}

// Non-null string arguments point to NUL-terminated strings. Handles come
// from the stage functions and remain owned by C until close succeeds.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_stage_bytes(
    target: *const c_char,
    candidate: *const c_void,
    len: usize,
    out: *mut *mut Transaction,
) -> c_int {
    if candidate.is_null() || len == 0 || len > isize::MAX as usize {
        return unsafe { stage(ptr::null(), out, true, |_| Ok(())) };
    }
    unsafe {
        stage(target, out, true, |tx| {
            let bytes = std::slice::from_raw_parts(candidate.cast::<u8>(), len);
            tx.stage(Candidate::Bytes(bytes))
        })
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_stage_file(
    target: *const c_char,
    candidate: *const c_char,
    out: *mut *mut Transaction,
) -> c_int {
    if candidate.is_null() {
        return unsafe { stage(ptr::null(), out, true, |_| Ok(())) };
    }
    let candidate = unsafe { CStr::from_ptr(candidate) };
    if candidate.to_bytes().is_empty() {
        return unsafe { stage(ptr::null(), out, true, |_| Ok(())) };
    }
    unsafe { stage(target, out, true, |tx| tx.staged_file(candidate)) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_stage_removal(
    target: *const c_char,
    out: *mut *mut Transaction,
) -> c_int {
    unsafe { stage(target, out, false, |_| Ok(())) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_commit(
    tx: *mut Transaction,
    validator: Option<Validator>,
    context: *mut c_void,
) -> c_int {
    if tx.is_null() {
        return Status::InvalidArgument as c_int;
    }
    if let Err(status) = unsafe { &mut *tx }.commit() {
        return status as c_int;
    }
    if let Some(validator) = validator {
        let path = unsafe { &*tx }.target.path.as_ptr();
        if !unsafe { validator(path, context) } {
            return unsafe { &mut *tx }.restore_after(Status::ValidationFailed)
                as c_int;
        }
    }
    Status::Ok as c_int
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_rollback(
    tx: *mut Transaction,
) -> c_int {
    status(
        unsafe { tx.as_mut() }
            .ok_or(Status::InvalidArgument)
            .and_then(Transaction::rollback),
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_finalize(
    tx: *mut Transaction,
) -> c_int {
    match unsafe { tx.as_mut() }
        .ok_or(Status::InvalidArgument)
        .and_then(Transaction::finalize)
    {
        Ok(status) | Err(status) => status as c_int,
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_close(
    tx: *mut *mut Transaction,
) -> c_int {
    let Some(out) = (unsafe { tx.as_mut() }) else {
        return Status::InvalidArgument as c_int;
    };
    let Some(transaction) = (unsafe { out.as_mut() }) else {
        return Status::InvalidArgument as c_int;
    };
    if let Err(status) = transaction.close() {
        return status as c_int;
    }
    // SAFETY: close completed and the caller transfers ownership of the handle back.
    drop(unsafe { Box::from_raw(*out) });
    *out = ptr::null_mut();
    Status::Ok as c_int
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_target_path(
    tx: *const Transaction,
) -> *const c_char {
    unsafe { tx.as_ref() }.map_or(ptr::null(), |tx| tx.target.path.as_ptr())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_staged_path(
    tx: *const Transaction,
) -> *const c_char {
    unsafe { tx.as_ref() }
        .and_then(|tx| tx.staged.as_ref())
        .map_or(ptr::null(), |asset| asset.entry.path.as_ptr())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_backup_path(
    tx: *const Transaction,
) -> *const c_char {
    unsafe { tx.as_ref() }
        .and_then(|tx| tx.backup.as_ref())
        .map_or(ptr::null(), |asset| asset.entry.path.as_ptr())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_deferred_path(
    tx: *const Transaction,
) -> *const c_char {
    unsafe { tx.as_ref() }
        .filter(|tx| tx.state == State::Deferred)
        .and_then(|tx| tx.backup.as_ref())
        .map_or(ptr::null(), |asset| asset.entry.path.as_ptr())
}

#[unsafe(no_mangle)]
extern "C" fn cbm_activation_transaction_status_message(
    status: c_int,
) -> *const c_char {
    match status {
        0 => c"activation transaction completed",
        1 => c"activation completed; old executable cleanup was deferred",
        -1 => c"invalid activation transaction argument",
        -2 => c"activation transaction allocation failed",
        -3 => c"activation transaction I/O failed",
        -4 => c"activation transaction is in the wrong state",
        -5 => c"activated candidate failed validation and was rolled back",
        -6 => c"activation failed and the retained backup could not be restored",
        _ => c"unknown activation transaction status",
    }
    .as_ptr()
}

#[unsafe(no_mangle)]
extern "C" fn cbm_activation_transaction_refusal_note() -> *const c_char {
    REFUSAL.with(|buffer| buffer.get().cast())
}

#[cfg(any(test, feature = "native-engine-test-seams"))]
#[unsafe(no_mangle)]
extern "C" fn cbm_activation_transaction_set_before_absent_publish_for_test(
    callback: Option<Callback>,
    context: *mut c_void,
) {
    BEFORE_ABSENT.with(|hook| hook.set((callback, context)));
}

#[cfg(any(test, feature = "native-engine-test-seams"))]
#[unsafe(no_mangle)]
extern "C" fn cbm_activation_transaction_rename_failures_set_for_test(
    count: u32,
) {
    platform::rename_failures(count);
}

#[cfg(any(test, feature = "native-engine-test-seams"))]
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_note_refusal_for_testing(
    predicate: *const c_char,
    error: c_ulong,
) {
    clear_note();
    if !predicate.is_null() {
        let text = unsafe { CStr::from_ptr(predicate) }.to_string_lossy();
        if !text.is_empty() {
            note(&text, &format!("os {error}"), None);
        }
    }
}

#[cfg(any(test, feature = "native-engine-test-seams"))]
#[unsafe(no_mangle)]
extern "C" fn cbm_activation_transaction_write_failures_set_for_testing(
    count: u32,
    error: c_int,
) {
    WRITE_FAILURE.with(|failure| failure.set((count, error)));
}

#[cfg(any(test, feature = "native-engine-test-seams"))]
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_set_home_root_for_testing(
    path: *const c_char,
    uid: c_ulong,
) {
    #[cfg(target_os = "linux")]
    HOME.with(|value| {
        *value.borrow_mut() = (!path.is_null()).then(|| {
            (
                unsafe { CStr::from_ptr(path) }.to_owned(),
                uid as libc::uid_t,
            )
        });
    });
    #[cfg(not(target_os = "linux"))]
    let _ = (path, uid);
}

#[cfg(all(
    any(target_os = "linux", target_os = "macos"),
    any(test, feature = "native-engine-test-seams")
))]
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_activation_transaction_map_alias_for_testing(
    link: *const c_char,
    owner: c_ulong,
    trusted: c_ulong,
    own_target: bool,
    rest: *const c_char,
) -> *mut c_char {
    if link.is_null() || rest.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe {
        platform::map_alias(
            CStr::from_ptr(link),
            owner as libc::uid_t,
            trusted as libc::uid_t,
            own_target,
            CStr::from_ptr(rest).to_bytes(),
        )
    };
    // SAFETY: the C test releases the returned allocation through the C allocator.
    result.map_or(ptr::null_mut(), |path| unsafe {
        libc::strdup(path.as_ptr())
    })
}
