//! Scoped adapter for Go's cross-file registry and staged result sink.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::slice;

use crate::resolver::{
    CallFact, CallableIdentity, Resolution, ResolveFailure,
    ResolveStatus as Status, ResolvedCall, SourceSite, copy_name,
};

// CBMResolvedCall from cbm.h.
#[repr(C)]
struct NativeCall {
    caller: *const c_char,
    callee: *const c_char,
    strategy: *const c_char,
    confidence: f32,
    reason: *const c_char,
    kind: u32,
    site: SourceSite,
    signature: *const c_char,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct NativeIdentity {
    qualified_name: *const c_char,
    signature: *const c_char,
}

#[repr(C)]
struct Bytes {
    data: *const u8,
    len: usize,
}

impl From<&CStr> for Bytes {
    fn from(value: &CStr) -> Self {
        Self {
            data: value.to_bytes().as_ptr(),
            len: value.count_bytes(),
        }
    }
}

#[repr(C)]
struct OutputRow {
    caller: Bytes,
    callee: Bytes,
    strategy: Bytes,
    signature: Bytes,
    confidence: f32,
    kind: u32,
    site: SourceSite,
}

impl From<&ResolvedCall> for OutputRow {
    fn from(row: &ResolvedCall) -> Self {
        Self {
            caller: row.fact.caller.as_c_str().into(),
            callee: row.target.qualified_name.as_c_str().into(),
            strategy: row.strategy.into(),
            signature: row.target.signature.as_deref().map_or(
                Bytes {
                    data: ptr::null(),
                    len: 0,
                },
                Bytes::from,
            ),
            confidence: row.confidence,
            kind: row.fact.kind,
            site: row.fact.site,
        }
    }
}

type Lookup = unsafe extern "C" fn(*mut c_void, name: Bytes) -> NativeIdentity;
type Cancelled = unsafe extern "C" fn(*mut c_void) -> bool;
type Stage = unsafe extern "C" fn(*mut c_void, *const OutputRow) -> bool;

struct ProjectContext<'generation> {
    package: Option<&'generation CStr>,
    import_names: &'generation [*const c_char],
    import_qns: &'generation [*const c_char],
    native: *mut c_void,
    lookup: Lookup,
    cancelled: Cancelled,
}

impl ProjectContext<'_> {
    fn is_cancelled(&self) -> bool {
        // SAFETY: The native caller lends the live resolve context.
        unsafe { (self.cancelled)(self.native) }
    }

    fn import(&self, alias: &[u8]) -> Option<&[u8]> {
        self.import_names
            .iter()
            .copied()
            .zip(self.import_qns.iter().copied())
            .find_map(|(name, qn)| {
                // SAFETY: The native caller lends two arrays of optional C strings.
                let (name, qn) =
                    unsafe { (optional_cstr(name)?, optional_cstr(qn)?) };
                (name.to_bytes() == alias).then_some(qn.to_bytes())
            })
    }

    fn bind(
        &self,
        row: &NativeCall,
    ) -> Result<Option<ResolvedCall>, ResolveFailure> {
        // SAFETY: The input row borrows optional C strings for the whole call.
        let (Some(caller), Some(callee), Some(strategy)) = (unsafe {
            (
                optional_cstr(row.caller),
                optional_cstr(row.callee),
                optional_cstr(row.strategy),
            )
        }) else {
            return Ok(None);
        };
        if strategy != c"lsp_unresolved" {
            return Ok(None);
        }
        let fact = CallFact {
            caller: copy_name(caller)?,
            spelling: copy_name(callee)?,
            site: row.site,
            kind: row.kind,
        };
        let lookup = |name: &CStr| {
            // SAFETY: The callback consumes the name synchronously and the
            // registry keeps its returned strings live for this generation.
            let identity = unsafe { (self.lookup)(self.native, name.into()) };
            (!identity.qualified_name.is_null()).then_some(identity)
        };
        let Some(target) = super::resolve(
            fact.spelling.as_c_str(),
            self.package.map(CStr::to_bytes),
            |name| self.import(name),
            lookup,
        ) else {
            return Ok(None);
        };
        // SAFETY: A successful lookup supplies a live qualified name and an
        // optional signature. Copy both while the generation is borrowed.
        let qualified_name =
            copy_name(unsafe { CStr::from_ptr(target.qualified_name) })?;
        let signature = unsafe { optional_cstr(target.signature) }
            .map(copy_name)
            .transpose()?;
        Ok(Some(ResolvedCall {
            fact,
            target: CallableIdentity {
                qualified_name,
                signature,
            },
            strategy: c"lsp_strategy_cross_file",
            confidence: 0.92,
        }))
    }

    fn resolve(&self, row: &NativeCall) -> Resolution<ResolvedCall> {
        if self.is_cancelled() {
            return Resolution::Cancelled;
        }
        match self.bind(row) {
            Ok(Some(call)) => Resolution::Resolved(call),
            Ok(None) => Resolution::Unresolved,
            Err(error) => Resolution::Failed(error),
        }
    }
}

unsafe fn optional_cstr<'a>(value: *const c_char) -> Option<&'a CStr> {
    // SAFETY: The native caller ties any present string to 'a.
    (!value.is_null()).then(|| unsafe { CStr::from_ptr(value) })
}

unsafe fn native_slice<'a, T>(items: *const T, count: usize) -> &'a [T] {
    if count == 0 {
        return &[];
    }
    // SAFETY: The native caller lends count live elements.
    unsafe { slice::from_raw_parts(items, count) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_resolve_metadata(
    rows: *const NativeCall,
    count: usize,
    package: *const c_char,
    import_names: *const *const c_char,
    import_qns: *const *const c_char,
    import_count: usize,
    native: *mut c_void,
    lookup: Lookup,
    cancelled: Cancelled,
    stage: Stage,
) -> Status {
    // SAFETY: The native context holds one read-only registry generation
    // and all input buffers live and immutable until the callback returns.
    let project = ProjectContext {
        package: unsafe { optional_cstr(package) },
        import_names: unsafe { native_slice(import_names, import_count) },
        import_qns: unsafe { native_slice(import_qns, import_count) },
        native,
        lookup,
        cancelled,
    };
    for row in unsafe { native_slice(rows, count) } {
        match project.resolve(row) {
            Resolution::Resolved(call) => {
                // SAFETY: Native staging copies output bytes before return.
                if !unsafe { stage(native, &OutputRow::from(&call)) } {
                    return Status::Failed;
                }
            }
            Resolution::Unresolved => {}
            Resolution::Cancelled => return Status::Cancelled,
            Resolution::Failed(ResolveFailure::Allocation) => {
                return Status::Failed;
            }
        }
    }
    if project.is_cancelled() {
        Status::Cancelled
    } else {
        Status::Complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sink {
        lookup: Vec<Vec<u8>>,
        staged: Vec<(Vec<u8>, SourceSite, u32)>,
        cancel_after: usize,
        fail_after: usize,
    }

    unsafe extern "C" fn lookup(
        context: *mut c_void,
        name: Bytes,
    ) -> NativeIdentity {
        // SAFETY: The test lends a live sink and the adapter lends the spelling.
        let sink = unsafe { &mut *context.cast::<Sink>() };
        let name = unsafe { slice::from_raw_parts(name.data, name.len) };
        sink.lookup.push(name.to_vec());
        let target = if name == b"first.Read" {
            c"first.Read".as_ptr()
        } else {
            ptr::null()
        };
        NativeIdentity {
            qualified_name: target,
            signature: ptr::null(),
        }
    }

    unsafe extern "C" fn cancelled(context: *mut c_void) -> bool {
        // SAFETY: The fixture keeps the sink live during resolution.
        let sink = unsafe { &*context.cast::<Sink>() };
        sink.staged.len() == sink.cancel_after
    }

    unsafe extern "C" fn stage(
        context: *mut c_void,
        row: *const OutputRow,
    ) -> bool {
        // SAFETY: The fixture lends a sink and resolution lends a live row.
        let sink = unsafe { &mut *context.cast::<Sink>() };
        if sink.staged.len() == sink.fail_after {
            return false;
        }
        let row = unsafe { &*row };
        let target =
            unsafe { slice::from_raw_parts(row.callee.data, row.callee.len) };
        sink.staged.push((target.to_vec(), row.site, row.kind));
        true
    }

    #[test]
    fn stages_sites_in_input_order_and_reports_interruption() {
        let call = |start, end, kind| NativeCall {
            caller: c"caller".as_ptr(),
            callee: c"api.Read".as_ptr(),
            strategy: c"lsp_unresolved".as_ptr(),
            confidence: 0.,
            reason: c"symbol_not_in_registry".as_ptr(),
            kind,
            site: SourceSite {
                start,
                end,
                origin: 0,
            },
            signature: ptr::null(),
        };
        let rows = [call(90, 98, 0), call(75, 85, 1)];
        let aliases = [
            c"api".as_ptr(),
            ptr::null(),
            c"api".as_ptr(),
            c"api".as_ptr(),
        ];
        let packages = [
            ptr::null(),
            c"ignored".as_ptr(),
            c"first".as_ptr(),
            c"second".as_ptr(),
        ];
        for (cancel_after, fail_after, expected, staged) in [
            (usize::MAX, usize::MAX, Status::Complete, 2),
            (1, usize::MAX, Status::Cancelled, 1),
            (usize::MAX, 1, Status::Failed, 1),
        ] {
            let mut sink = Sink {
                lookup: Vec::new(),
                staged: Vec::new(),
                cancel_after,
                fail_after,
            };
            // SAFETY: Both arrays and all strings outlive the synchronous call.
            let status = unsafe {
                mori_go_resolve_metadata(
                    rows.as_ptr(),
                    rows.len(),
                    ptr::null(),
                    aliases.as_ptr(),
                    packages.as_ptr(),
                    aliases.len(),
                    (&raw mut sink).cast(),
                    lookup,
                    cancelled,
                    stage,
                )
            };
            assert_eq!(status as u32, expected as u32);
            assert_eq!(sink.staged.len(), staged);
            for (output, input) in sink.staged.iter().zip(&rows) {
                assert_eq!(
                    output,
                    &(b"first.Read".to_vec(), input.site, input.kind)
                );
            }
            assert_eq!(
                sink.lookup[0..2],
                [b"api.Read".to_vec(), b"first.Read".to_vec()]
            );
            assert!(!sink.lookup.iter().any(|key| key == b"second.Read"));
        }
    }

    #[test]
    fn missing_target_without_imports_is_a_completed_pass() {
        let row = NativeCall {
            caller: c"caller".as_ptr(),
            callee: c"Missing".as_ptr(),
            strategy: c"lsp_unresolved".as_ptr(),
            confidence: 0.,
            reason: c"function_not_in_registry".as_ptr(),
            kind: 0,
            site: SourceSite {
                start: 20,
                end: 30,
                origin: 0,
            },
            signature: ptr::null(),
        };
        let mut sink = Sink {
            lookup: Vec::new(),
            staged: Vec::new(),
            cancel_after: usize::MAX,
            fail_after: usize::MAX,
        };
        // SAFETY: One live row, and zero count permits null import arrays.
        let status = unsafe {
            mori_go_resolve_metadata(
                &row,
                1,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0,
                (&raw mut sink).cast(),
                lookup,
                cancelled,
                stage,
            )
        };
        assert!(matches!(status, Status::Complete));
        assert_eq!(sink.lookup, [b"Missing".to_vec()]);
        assert!(sink.staged.is_empty());
    }
}
