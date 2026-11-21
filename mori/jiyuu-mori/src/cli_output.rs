//! Process-level flags for one-shot CLI output.

use std::ffi::{CStr, c_char, c_int};

/// ABI mirror of `cbm_cli_output_flags_t` in `progress_sink.h`.
#[derive(Default)]
#[repr(C)]
struct OutputFlags {
    quiet_requested: bool,
    progress_requested: bool,
    verbose_requested: bool,
}

/// Each non-null slot must reference a live NUL-terminated argument.
unsafe fn compact(argv: &mut [*mut c_char]) -> (c_int, OutputFlags) {
    let mut flags = OutputFlags::default();
    let mut before_tool = true;
    let mut output_index = 0;
    for input_index in 0..argv.len() {
        let ptr = argv[input_index];
        // SAFETY: The process supplies NUL-terminated argument strings.
        // A null slot is retained without reading it.
        let argument =
            (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr).to_bytes() });
        match argument {
            Some(b"--quiet") => flags.quiet_requested = true,
            Some(b"--progress") => flags.progress_requested = true,
            // Tools such as index_status own their own --verbose field.
            Some(b"--verbose") if before_tool => flags.verbose_requested = true,
            _ => {
                argv[output_index] = ptr;
                output_index += 1;
                if let Some(argument) = argument {
                    before_tool &= !argument.is_empty() && argument[0] == b'-';
                }
            }
        }
    }
    (output_index as c_int, flags)
}

#[cfg(all(feature = "c-backend", not(test)))]
type Compactor =
    unsafe extern "C" fn(*mut c_int, *mut *mut c_char, *mut OutputFlags);

#[cfg(all(feature = "c-backend", not(test)))]
unsafe extern "C" {
    fn cbm_cli_set_output_compactor(compact: Compactor);
}

#[cfg(all(feature = "c-backend", not(test)))]
unsafe extern "C" fn compact_args(
    argc: *mut c_int,
    argv: *mut *mut c_char,
    flags: *mut OutputFlags,
) {
    // SAFETY: The C caller validates all three pointers. Its argc covers the
    // mutable slots in argv, and flags points to its result struct.
    let args =
        unsafe { std::slice::from_raw_parts_mut(argv, (*argc).max(0) as usize) };
    let (count, parsed) = unsafe { compact(args) };
    unsafe {
        *argc = count;
        *flags = parsed;
    }
}

#[cfg(all(feature = "c-backend", not(test)))]
pub(crate) unsafe fn register() {
    // SAFETY: Registered before C main starts. The callback is static.
    unsafe { cbm_cli_set_output_compactor(compact_args) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_tool_verbose_and_compacts_global_controls() {
        let kept = [
            c"index_status".as_ptr().cast_mut(),
            c"--verbose".as_ptr().cast_mut(),
        ];
        let mut argv = [
            c"--verbose".as_ptr().cast_mut(),
            kept[0],
            c"--progress".as_ptr().cast_mut(),
            kept[1],
        ];
        // SAFETY: Every slot points to a static C string.
        let (count, flags) = unsafe { compact(&mut argv) };
        assert_eq!(&argv[..count as usize], &kept);
        assert!(
            flags.progress_requested
                && flags.verbose_requested
                && !flags.quiet_requested
        );
    }

    #[test]
    fn null_is_retained_and_empty_argument_counts_as_the_tool() {
        let kept = [
            std::ptr::null_mut(),
            c"".as_ptr().cast_mut(),
            c"--verbose".as_ptr().cast_mut(),
        ];
        let mut argv =
            [kept[0], kept[1], kept[2], c"--quiet".as_ptr().cast_mut()];
        // SAFETY: Every non-null slot points to a static C string.
        let (count, flags) = unsafe { compact(&mut argv) };
        assert_eq!(&argv[..count as usize], &kept);
        assert!(
            flags.quiet_requested
                && !flags.progress_requested
                && !flags.verbose_requested
        );
    }
}
