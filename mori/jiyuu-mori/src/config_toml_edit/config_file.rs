//! File operations are shared with the managed text editor. The TOML test
//! hooks are independent and run at the same publication phases as in C.

use std::io;
use std::path::Path;

use crate::config_text_edit::file::{Snapshot, Target};

#[cfg(feature = "native-engine-test-seams")]
mod test_hooks {
    use std::cell::Cell;
    use std::ffi::{CString, c_char, c_void};
    use std::path::Path;

    use crate::config_text_edit::file::WritePhase;

    type Hook = Option<unsafe extern "C" fn(*const c_char, *mut c_void)>;
    type Slot = Cell<(Hook, *mut c_void)>;

    thread_local! {
        static PRECOMMIT: Slot = const { Cell::new((None, std::ptr::null_mut())) };
        static PREPUBLISH: Slot = const { Cell::new((None, std::ptr::null_mut())) };
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cbm_toml_set_precommit_hook_for_testing(
        hook: Hook,
        context: *mut c_void,
    ) {
        PRECOMMIT.set((hook, context));
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cbm_toml_set_prepublish_hook_for_testing(
        hook: Hook,
        context: *mut c_void,
    ) {
        PREPUBLISH.set((hook, context));
    }

    pub(super) fn run(phase: WritePhase, path: &Path) {
        let slot = match phase {
            WritePhase::Precommit => &PRECOMMIT,
            WritePhase::Prepublish => &PREPUBLISH,
            #[allow(unreachable_patterns)]
            _ => return,
        };
        let (hook, context) = slot.get();
        if let Some(hook) = hook {
            let Ok(path) = CString::new(path.as_os_str().as_encoded_bytes())
            else {
                return;
            };
            // SAFETY: the caller registered this hook with its context, and the
            // encoded path remains valid throughout the call.
            unsafe { hook(path.as_ptr(), context) };
        }
    }
}

pub(super) struct ConfigFile<'a> {
    target: Target<'a>,
    snapshot: Snapshot,
}

impl<'a> ConfigFile<'a> {
    pub fn read(path: &'a Path) -> io::Result<Self> {
        let target = Target::open_validated(path)?;
        let snapshot = target.read()?;
        Ok(Self { target, snapshot })
    }

    pub fn bytes(&self) -> &[u8] {
        self.snapshot.content().unwrap_or_default()
    }

    pub fn write(&self, data: &[u8]) -> io::Result<()> {
        #[cfg(feature = "native-engine-test-seams")]
        let run = test_hooks::run;
        #[cfg(not(feature = "native-engine-test-seams"))]
        let run = |_, _: &Path| {};
        self.target.write_toml(&self.snapshot, data, run)
    }
}
