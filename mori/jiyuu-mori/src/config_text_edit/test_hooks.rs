//! Test-only race hooks shared by the Rust tests and the direct C fixtures.

use std::cell::Cell;
#[cfg(test)]
use std::cell::RefCell;
use std::ffi::{CString, c_char, c_void};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Precommit,
    Prepublish,
    TempClosed,
}

type Callback = unsafe extern "C" fn(*const c_char, *mut c_void);
type CRegistration = (Option<Callback>, *mut c_void);
#[cfg(test)]
type RustHook = (Phase, Box<dyn FnOnce(&Path)>);

thread_local! {
    static C_HOOKS: Cell<[CRegistration; 3]> = const {
        Cell::new([(None, std::ptr::null_mut()); 3])
    };
    #[cfg(test)]
    static RUST_HOOK: RefCell<Option<RustHook>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn hook(phase: Phase, action: impl FnOnce(&Path) + 'static) {
    RUST_HOOK.with(|slot| {
        assert!(slot.replace(Some((phase, Box::new(action)))).is_none())
    });
}

pub(super) fn run_hook(phase: Phase, path: &Path) {
    #[cfg(test)]
    {
        let action = RUST_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|(at, _)| *at == phase) {
                slot.take().map(|(_, action)| action)
            } else {
                None
            }
        });
        if let Some(action) = action {
            action(path);
        }
    }
    let (callback, context) = C_HOOKS.with(|hooks| hooks.get()[phase as usize]);
    if let Some(callback) = callback
        && let Ok(path) = CString::new(path.as_os_str().as_encoded_bytes())
    {
        // SAFETY: C registered the callback and owns its context until reset.
        unsafe { callback(path.as_ptr(), context) };
    }
}

fn set(phase: Phase, callback: Option<Callback>, context: *mut c_void) {
    C_HOOKS.with(|hooks| {
        let mut items = hooks.get();
        items[phase as usize] = (callback, context);
        hooks.set(items);
    });
}

#[unsafe(no_mangle)]
extern "C" fn cbm_text_set_precommit_hook_for_testing(
    hook: Option<Callback>,
    context: *mut c_void,
) {
    set(Phase::Precommit, hook, context);
}

#[unsafe(no_mangle)]
extern "C" fn cbm_text_set_prepublish_hook_for_testing(
    hook: Option<Callback>,
    context: *mut c_void,
) {
    set(Phase::Prepublish, hook, context);
}

#[unsafe(no_mangle)]
extern "C" fn cbm_text_set_temp_closed_hook_for_testing(
    hook: Option<Callback>,
    context: *mut c_void,
) {
    set(Phase::TempClosed, hook, context);
}
