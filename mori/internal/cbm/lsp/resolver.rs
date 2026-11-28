//! Owned call facts and outcomes used by language resolvers.

use std::collections::TryReserveError;
use std::ffi::{CStr, CString};

/// Offsets are byte indices into the raw or preprocessed source identified
/// by `origin`. Legacy facts can have no occurrence (end <= start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct SourceSite {
    pub start: u32,
    pub end: u32,
    pub origin: u32,
}

/// Callable identity is independent of the spelling at a source site.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CallableIdentity {
    pub qualified_name: CString,
    pub signature: Option<CString>,
}

pub(crate) struct CallFact {
    pub caller: CString,
    pub spelling: CString,
    pub site: SourceSite,
    pub kind: u32,
}

pub(crate) struct ResolvedCall {
    pub fact: CallFact,
    pub target: CallableIdentity,
    pub strategy: &'static CStr,
    pub confidence: f32,
}

pub(crate) enum Resolution<T> {
    Resolved(T),
    Unresolved,
    Cancelled,
    Failed(ResolveFailure),
}

pub(crate) enum ResolveFailure {
    Allocation,
}

impl From<TryReserveError> for ResolveFailure {
    fn from(_: TryReserveError) -> Self {
        Self::Allocation
    }
}

/// Copy C strings before handing facts or results across worker stages.
pub(crate) fn copy_name(name: &CStr) -> Result<CString, ResolveFailure> {
    let mut owned = Vec::new();
    owned.try_reserve_exact(name.to_bytes_with_nul().len())?;
    owned.extend_from_slice(name.to_bytes_with_nul());
    // SAFETY: The bytes are copied from a valid C string including its NUL.
    Ok(unsafe { CString::from_vec_with_nul_unchecked(owned) })
}

/// Completion of one file's resolution. Unresolved occurrences remain facts
/// within a completed file and never stand in for interruption.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub(crate) enum ResolveStatus {
    Complete,
    Cancelled,
    Failed,
}
