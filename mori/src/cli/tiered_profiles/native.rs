use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Write;
use std::ptr::NonNull;

use super::{PATH_CAPACITY, sibling_path};
use crate::{ffi_output, stdio::c_transport::FileStream};

// Tier, access, and dialect values from cli/agent_profiles.h.
const TIERS: [c_int; 3] = [0, 1, 2];
const VERIFY: c_int = 1;
const DIRECT: c_int = 0;
const HANDOFF: c_int = 1;
const CODEX: c_int = 1;
const OPENCODE: c_int = 5;

// These types mirror the private installer ABI in cli/cli.c.
#[repr(C)]
struct Profiles {
    label: *const c_char,
    verify_path: *const c_char,
    binary_path: *const c_char,
    legacy_verify_content: *const c_char,
    dialect: c_int,
    force_handoff: bool,
}

#[repr(C)]
#[allow(dead_code)] // The C installer constructs these values.
enum Action {
    Plan,
    PreviewInstall,
    Install,
    PreviewUninstall,
    Uninstall,
}

impl Action {
    fn uninstalling(&self) -> bool {
        matches!(self, Self::PreviewUninstall | Self::Uninstall)
    }
}

#[repr(C)]
struct Services {
    record_plan:
        unsafe extern "C" fn(*const c_char, *const c_char, *const c_char),
    record_error:
        unsafe extern "C" fn(bool, *const c_char, *const c_char, *const c_char),
    prepare_parent: unsafe extern "C" fn(*const c_char) -> bool,
    free_rendered: unsafe extern "C" fn(*mut c_void),
}

unsafe extern "C" {
    fn cbm_graph_tier_slug(tier: c_int) -> *const c_char;
    fn cbm_graph_dialect_direct_capable(dialect: c_int) -> bool;
    fn cbm_render_graph_profile(
        dialect: c_int,
        tier: c_int,
        access: c_int,
        binary_path: *const c_char,
    ) -> *mut c_char;
    fn cbm_render_graph_profile_legacy(
        dialect: c_int,
        tier: c_int,
        access: c_int,
        binary_path: *const c_char,
    ) -> *mut c_char;
    fn cbm_render_graph_profile_codex_rc1(tier: c_int) -> *mut c_char;
    fn cbm_render_graph_profile_opencode_v0108(
        tier: c_int,
        access: c_int,
    ) -> *mut c_char;
    fn cbm_render_graph_prompt(tier: c_int, access: c_int) -> *mut c_char;
    fn cbm_render_graph_prompt_legacy(tier: c_int, access: c_int)
    -> *mut c_char;
    fn cbm_text_migrate_owned_document(
        path: *const c_char,
        current: *const c_char,
        released: *const *const c_char,
        count: usize,
    ) -> c_int;
    fn cbm_text_remove_owned_document_any(
        path: *const c_char,
        current: *const c_char,
        released: *const *const c_char,
        count: usize,
    ) -> c_int;

}

struct Rendered {
    content: NonNull<c_char>,
    free: unsafe extern "C" fn(*mut c_void),
}

impl Rendered {
    fn new(content: *mut c_char, services: &Services) -> Option<Self> {
        Some(Self {
            content: NonNull::new(content)?,
            free: services.free_rendered,
        })
    }

    fn as_ptr(&self) -> *const c_char {
        self.content.as_ptr()
    }
}

impl Drop for Rendered {
    fn drop(&mut self) {
        // SAFETY: The renderer allocated this string. Use its native allocator.
        unsafe { (self.free)(self.content.as_ptr().cast()) };
    }
}

#[derive(Clone, Copy)]
enum Document {
    Agent,
    Prompt,
}

impl Document {
    fn kind(self) -> &'static CStr {
        match self {
            Self::Agent => c"agent",
            Self::Prompt => c"prompt",
        }
    }

    fn operations(self) -> [&'static CStr; 4] {
        match self {
            Self::Agent => [
                c"agent_path",
                c"agent_render",
                c"agent_install",
                c"agent_uninstall",
            ],
            Self::Prompt => [
                c"prompt_path",
                c"prompt_render",
                c"prompt_install",
                c"prompt_uninstall",
            ],
        }
    }

    // SAFETY: The caller keeps the native input strings alive during render.
    unsafe fn render(
        self,
        profiles: &Profiles,
        services: &Services,
        tier: c_int,
        access: c_int,
    ) -> Option<Rendered> {
        let content = unsafe {
            match self {
                Self::Agent => cbm_render_graph_profile(
                    profiles.dialect,
                    tier,
                    access,
                    profiles.binary_path,
                ),
                Self::Prompt => cbm_render_graph_prompt(tier, access),
            }
        };
        Rendered::new(content, services)
    }

    // SAFETY: The caller keeps the native input strings alive during render.
    unsafe fn released(
        self,
        profiles: &Profiles,
        services: &Services,
        tier: c_int,
        access: c_int,
    ) -> Vec<Rendered> {
        let mut versions = Vec::new();
        let mut keep =
            |content| versions.extend(Rendered::new(content, services));
        let alternate = if access == DIRECT { HANDOFF } else { DIRECT };
        // Keep the release renderings in the same order as the installer used.
        unsafe {
            match self {
                Self::Prompt => {
                    keep(cbm_render_graph_prompt_legacy(tier, access))
                }
                Self::Agent => {
                    let dialect = profiles.dialect;
                    let binary = profiles.binary_path;
                    keep(cbm_render_graph_profile(
                        dialect, tier, alternate, binary,
                    ));
                    keep(cbm_render_graph_profile_legacy(
                        dialect, tier, access, binary,
                    ));
                    keep(cbm_render_graph_profile_legacy(
                        dialect, tier, alternate, binary,
                    ));
                    if dialect == CODEX && access == DIRECT {
                        keep(cbm_render_graph_profile_codex_rc1(tier));
                    }
                    if dialect == OPENCODE {
                        keep(cbm_render_graph_profile_opencode_v0108(
                            tier, access,
                        ));
                        keep(cbm_render_graph_profile_opencode_v0108(
                            tier, alternate,
                        ));
                    }
                }
            }
        }
        versions
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_tiered_profiles(
    profiles: &Profiles,
    services: &Services,
    action: Action,
    prompts: bool,
    out: *mut c_void,
) {
    // SAFETY: The installer owns the input strings and callbacks for this call.
    // The editor borrows the current and released renderings synchronously.
    unsafe {
        let mut out = FileStream(out);
        let mut write_stdout = |parts: &[&[u8]]| {
            // Match a single printf to the inherited CRT stream.
            let _ = out.write_all(&parts.concat());
        };
        let document = if prompts {
            Document::Prompt
        } else {
            Document::Agent
        };
        let kind = document.kind();
        let [path_op, render_op, install_op, uninstall_op] =
            document.operations();
        let label = CStr::from_ptr(profiles.label).to_bytes();
        let verify = if profiles.verify_path.is_null() {
            b"".as_slice()
        } else {
            CStr::from_ptr(profiles.verify_path).to_bytes()
        };
        let uninstalling = action.uninstalling();
        let error = |operation: &CStr, path| {
            (services.record_error)(
                uninstalling,
                profiles.label,
                ffi_output::c_str(operation),
                path,
            );
        };
        let access = if !profiles.force_handoff
            && cbm_graph_dialect_direct_capable(profiles.dialect)
        {
            DIRECT
        } else {
            HANDOFF
        };

        for tier in TIERS {
            let mut path_buffer = [0; PATH_CAPACITY];
            let slug = CStr::from_ptr(cbm_graph_tier_slug(tier));
            let Some(path) =
                sibling_path(verify, slug.to_bytes(), &mut path_buffer)
            else {
                error(path_op, profiles.verify_path);
                continue;
            };
            let path_ptr = ffi_output::c_str(path);
            let path = path.to_bytes();
            let kind_bytes = kind.to_bytes();

            match action {
                Action::Plan => (services.record_plan)(
                    profiles.label,
                    ffi_output::c_str(kind),
                    path_ptr,
                ),
                Action::PreviewInstall => {
                    write_stdout(&[b"  ", kind_bytes, b": ", path, b"\n"])
                }
                Action::PreviewUninstall => write_stdout(&[
                    b"  ",
                    label,
                    b" ",
                    kind_bytes,
                    b": would remove owned profile ",
                    path,
                    b"\n",
                ]),
                Action::Install | Action::Uninstall => {
                    let Some(current) =
                        document.render(profiles, services, tier, access)
                    else {
                        error(render_op, path_ptr);
                        continue;
                    };
                    let released =
                        document.released(profiles, services, tier, access);
                    let mut contents: Vec<_> =
                        released.iter().map(Rendered::as_ptr).collect();
                    if tier == VERIFY
                        && !profiles.legacy_verify_content.is_null()
                    {
                        contents.push(profiles.legacy_verify_content);
                    }
                    let result = if uninstalling {
                        cbm_text_remove_owned_document_any(
                            path_ptr,
                            current.as_ptr(),
                            contents.as_ptr(),
                            contents.len(),
                        )
                    } else if (services.prepare_parent)(path_ptr) {
                        cbm_text_migrate_owned_document(
                            path_ptr,
                            current.as_ptr(),
                            contents.as_ptr(),
                            contents.len(),
                        )
                    } else {
                        -1
                    };
                    drop(released);
                    drop(current);

                    if uninstalling {
                        if result < 0 {
                            error(uninstall_op, path_ptr);
                        } else {
                            let message = if result > 0 {
                                b": preserved modified profile ".as_slice()
                            } else {
                                b": removed owned profile "
                            };
                            write_stdout(&[
                                b"  ", label, b" ", kind_bytes, message, path,
                                b"\n",
                            ]);
                        }
                    } else {
                        if result >= 0 {
                            let message = if result > 0 {
                                b": preserved modified profile ".as_slice()
                            } else {
                                b": "
                            };
                            write_stdout(&[
                                b"  ", kind_bytes, message, path, b"\n",
                            ]);
                        }
                        if result != 0 {
                            error(install_op, path_ptr);
                        }
                    }
                }
            }
        }
    }
}
