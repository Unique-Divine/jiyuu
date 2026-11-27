//! Choose provider repair work from a closed graph snapshot.

use std::ffi::CStr;

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Pending,
    Settled,
    Repair,
    Reconcile,
    WildcardBlocked(&'static CStr),
}

enum Generation {
    Current,
    Missing,
    Changed,
    Unknown,
}

fn decide_named(
    peers: impl Iterator<Item = Option<Generation>>,
    reverse_pending: bool,
) -> Decision {
    let mut missing = false;
    let mut stale = false;
    for peer in peers {
        match peer {
            Some(Generation::Current) => {}
            Some(Generation::Missing) => missing = true,
            Some(Generation::Changed | Generation::Unknown) => stale = true,
            None => return Decision::Pending,
        }
    }
    if missing {
        Decision::Pending
    } else if stale {
        Decision::Repair
    } else if reverse_pending {
        Decision::Reconcile
    } else {
        Decision::Settled
    }
}

fn decide_wildcard(peers: impl Iterator<Item = Generation>) -> Decision {
    let mut missing = false;
    for peer in peers {
        match peer {
            Generation::Current => {}
            Generation::Missing => missing = true,
            Generation::Unknown => {
                return Decision::WildcardBlocked(c"wildcard_unknown_peer");
            }
            Generation::Changed => {
                return Decision::WildcardBlocked(
                    c"wildcard_generation_changed",
                );
            }
        }
    }
    if missing {
        Decision::Pending
    } else {
        Decision::Reconcile
    }
}

#[cfg(feature = "native-engine-abi")]
mod native {
    use super::{CStr, Decision, Generation, decide_named, decide_wildcard};
    use std::ffi::{c_char, c_int};

    #[repr(C)]
    struct Application {
        _opaque: [u8; 0],
    }

    #[repr(C)]
    struct JsonValue {
        _opaque: [u8; 0],
    }

    #[repr(C)]
    struct PeerIter {
        _opaque: [u8; 0],
    }

    #[repr(C)]
    struct ProjectOps {
        target_at:
            unsafe extern "C" fn(*const JsonValue, usize) -> *mut JsonValue,
        lookup: unsafe extern "C" fn(
            *const JsonValue,
            *const c_char,
        ) -> *mut JsonValue,
        string: unsafe extern "C" fn(*const JsonValue) -> *const c_char,
        length: unsafe extern "C" fn(*const JsonValue) -> usize,
        next_peer: unsafe extern "C" fn(
            *mut PeerIter,
            *mut *const c_char,
        ) -> *const c_char,
        valid_name: unsafe extern "C" fn(*const c_char) -> bool,
        read_generation:
            unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> bool,
        match_saved: unsafe extern "C" fn(
            *mut Application,
            *const c_char,
            *const c_char,
            c_int,
        ) -> bool,
        reconcile: unsafe extern "C" fn(*mut Application, *const c_char) -> bool,
        skip: unsafe extern "C" fn(*const c_char, *const c_char),
        max_targets: usize,
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn mori_daemon_repair_recover(
        application: *mut Application,
        project: *const c_char,
        targets: *const JsonValue,
        target_count: usize,
        observations: *const JsonValue,
        observed_peers: *mut PeerIter,
        reverse_pending: bool,
        ops: &ProjectOps,
    ) -> bool {
        // SAFETY: The caller holds the wildcard lease, has closed the read scope,
        // and lends a live application and its matching callbacks for this call.
        unsafe {
            match decide_project(
                project,
                targets,
                target_count,
                observations,
                observed_peers,
                reverse_pending,
                ops,
            ) {
                Decision::Pending => false,
                Decision::Settled => true,
                Decision::Repair => (ops.match_saved)(
                    application,
                    c"recovery".as_ptr(),
                    project,
                    target_count as c_int,
                ),
                Decision::Reconcile => (ops.reconcile)(application, project),
                Decision::WildcardBlocked(reason) => {
                    (ops.skip)(project, reason.as_ptr());
                    true
                }
            }
        }
    }

    unsafe fn decide_project(
        project: *const c_char,
        targets: *const JsonValue,
        target_count: usize,
        observations: *const JsonValue,
        observed_peers: *mut PeerIter,
        reverse_pending: bool,
        ops: &ProjectOps,
    ) -> Decision {
        // CBM_GRAPH_COMPARE_GENERATION_SIZE in store.h.
        const GENERATION_CAPACITY: usize = 128;

        if target_count > ops.max_targets {
            return Decision::Pending;
        }

        // SAFETY: C lends the project name, yyjson documents, and their iterator
        // for this call. The callbacks accept nullable JSON values. They return
        // NUL-terminated strings or null, and write a terminated generation on success.
        unsafe {
            let read_generation = |name, observed: *const c_char| {
                let mut current = [0; GENERATION_CAPACITY];
                if !(ops.read_generation)(
                    name,
                    current.as_mut_ptr(),
                    current.len(),
                ) {
                    Generation::Missing
                } else if observed.is_null() {
                    Generation::Unknown
                } else if CStr::from_ptr(observed)
                    == CStr::from_ptr(current.as_ptr())
                {
                    Generation::Current
                } else {
                    Generation::Changed
                }
            };

            if target_count == 1 {
                let target = (ops.target_at)(targets, 0);
                let name = (ops.string)(target);
                if !name.is_null()
                    && (ops.length)(target) == 1
                    && CStr::from_ptr(name) == c"*"
                {
                    if !reverse_pending {
                        return Decision::Settled;
                    }
                    let peers = std::iter::from_fn(|| {
                        let mut observed = std::ptr::null();
                        let name =
                            (ops.next_peer)(observed_peers, &mut observed);
                        if name.is_null() {
                            None
                        } else if observed.is_null() {
                            Some(Generation::Unknown)
                        } else {
                            Some(read_generation(name, observed))
                        }
                    });
                    return decide_wildcard(peers);
                }
            }

            let project = CStr::from_ptr(project);
            let peers = (0..target_count).map(|index| {
                let target = (ops.target_at)(targets, index);
                let name = (ops.string)(target);
                if name.is_null()
                    || CStr::from_ptr(name).to_bytes().len()
                        != (ops.length)(target)
                    || !(ops.valid_name)(name)
                {
                    return None;
                }
                if CStr::from_ptr(name) == project {
                    return Some(Generation::Current);
                }
                let observed = (ops.string)((ops.lookup)(observations, name));
                Some(read_generation(name, observed))
            });
            decide_named(peers, reverse_pending)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Decision::*, Generation::*, *};

    #[test]
    fn named_missing_peer_defers_stale_selection_in_either_order() {
        for peers in [[Changed, Missing], [Missing, Changed], [Unknown, Missing]]
        {
            assert_eq!(decide_named(peers.into_iter().map(Some), true), Pending);
        }
    }

    #[test]
    fn named_repair_precedes_reverse_reconciliation() {
        for stale in [Changed, Unknown] {
            assert_eq!(decide_named([Some(stale)].into_iter(), true), Repair);
        }
        assert_eq!(
            decide_named([Some(Current), None].into_iter(), true),
            Pending
        );
        assert_eq!(decide_named([Some(Current)].into_iter(), true), Reconcile);
        assert_eq!(decide_named(std::iter::empty(), false), Settled);
    }

    #[test]
    fn wildcard_uses_first_definitive_block_in_observation_order() {
        assert_eq!(
            decide_wildcard([Missing, Unknown, Changed].into_iter()),
            WildcardBlocked(c"wildcard_unknown_peer")
        );
        assert_eq!(
            decide_wildcard([Missing, Changed, Unknown].into_iter()),
            WildcardBlocked(c"wildcard_generation_changed")
        );
        assert_eq!(decide_wildcard([Current, Missing].into_iter()), Pending);
        assert_eq!(decide_wildcard([Current].into_iter()), Reconcile);
        assert_eq!(decide_wildcard(std::iter::empty()), Reconcile);
    }
}
