//! File and byte accounting for the production Rust discovery walker.

use std::ffi::c_int;

#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct Limit {
    pub enabled: bool,
    pub value: u64,
}

#[repr(C)]
pub(super) struct Limits {
    pub deadline_ms: u64,
    pub max_files: c_int,
    pub resource_files: Limit,
    pub resource_bytes: Limit,
}

#[derive(Default, Debug, Eq, PartialEq)]
#[repr(C)]
pub(super) struct Violation {
    pub resource: c_int,
    pub observed: u64,
    pub limit: u64,
}

// cbm_index_resource_t. A zero resource means the bounded count ceiling.
pub(super) const BOUNDED_COUNT: c_int = 0;
const RESOURCE_FILES: c_int = 1;
const RESOURCE_SOURCE_BYTES: c_int = 2;

pub(super) struct Budget {
    pub limits: Limits,
    count: u64,
    source_files: u64,
    source_bytes: u64,
}

impl Budget {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            count: 0,
            source_files: 0,
            source_bytes: 0,
        }
    }

    pub fn deadline_expired(&self, now_ms: impl FnOnce() -> u64) -> bool {
        self.limits.deadline_ms != 0 && now_ms() >= self.limits.deadline_ms
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn admit(
        &mut self,
        size: i64,
        max_extract_bytes: impl FnOnce() -> u64,
    ) -> Result<(), Violation> {
        if self.limits.max_files >= 0
            && self.count >= self.limits.max_files as u64
        {
            return Err(Violation {
                resource: BOUNDED_COUNT,
                observed: self.count + 1,
                limit: self.limits.max_files as u64,
            });
        }

        let files = self.limits.resource_files;
        let bytes = self.limits.resource_bytes;
        let size = size.max(0) as u64;
        // Oversized source files remain in discovery. Extraction reports
        // them separately, so they only contribute to the bounded count.
        if (files.enabled || bytes.enabled) && size <= max_extract_bytes() {
            if files.enabled && self.source_files >= files.value {
                return Err(Violation {
                    resource: RESOURCE_FILES,
                    observed: self.source_files + 1,
                    limit: files.value,
                });
            }
            if bytes.enabled {
                let observed = self.source_bytes.saturating_add(size);
                if size > bytes.value - self.source_bytes {
                    return Err(Violation {
                        resource: RESOURCE_SOURCE_BYTES,
                        observed,
                        limit: bytes.value,
                    });
                }
                self.source_bytes = observed;
            }
            self.source_files += 1;
        }
        self.count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(
        max_files: c_int,
        resource_files: Option<u64>,
        resource_bytes: Option<u64>,
    ) -> Budget {
        let limit = |value: Option<u64>| Limit {
            enabled: value.is_some(),
            value: value.unwrap_or_default(),
        };
        Budget::new(Limits {
            deadline_ms: 0,
            max_files,
            resource_files: limit(resource_files),
            resource_bytes: limit(resource_bytes),
        })
    }

    #[test]
    fn bounded_count_takes_precedence_and_allows_exact_ceiling() {
        let mut budget = budget(1, Some(1), Some(4));
        assert_eq!(budget.admit(4, || 16), Ok(()));
        assert_eq!(
            budget.admit(1, || panic!("bounded count already refused")),
            Err(Violation {
                resource: BOUNDED_COUNT,
                observed: 2,
                limit: 1,
            })
        );
    }

    #[test]
    fn resource_file_limit_takes_precedence_over_bytes() {
        let mut budget = budget(-1, Some(1), Some(4));
        assert_eq!(budget.admit(4, || 16), Ok(()));
        assert_eq!(
            budget.admit(4, || 16),
            Err(Violation {
                resource: RESOURCE_FILES,
                observed: 2,
                limit: 1,
            })
        );
    }

    #[test]
    fn oversized_file_only_consumes_the_bounded_count() {
        let mut budget = budget(2, Some(1), Some(4));
        assert_eq!(budget.admit(5, || 4), Ok(()));
        assert_eq!(budget.admit(4, || 4), Ok(()));
        let violation = budget.admit(0, || 4).unwrap_err();
        assert_eq!(violation.resource, BOUNDED_COUNT);
        assert_eq!(violation.observed, 3);
    }

    #[test]
    fn empty_file_fits_a_zero_byte_limit() {
        let mut budget = budget(-1, None, Some(0));
        assert_eq!(budget.admit(0, || 4), Ok(()));
        assert_eq!(
            budget.admit(2, || 4),
            Err(Violation {
                resource: RESOURCE_SOURCE_BYTES,
                observed: 2,
                limit: 0,
            })
        );
    }

    #[test]
    fn byte_limit_checks_beyond_u64_without_wrapping_the_diagnostic() {
        let mut budget = budget(-1, None, Some(u64::MAX));
        assert_eq!(budget.admit(i64::MAX, || u64::MAX), Ok(()));
        assert_eq!(budget.admit(i64::MAX, || u64::MAX), Ok(()));
        assert_eq!(budget.admit(1, || u64::MAX), Ok(()));
        assert_eq!(
            budget.admit(1, || u64::MAX),
            Err(Violation {
                resource: RESOURCE_SOURCE_BYTES,
                observed: u64::MAX,
                limit: u64::MAX,
            })
        );
    }

    #[test]
    fn deadline_is_inclusive_and_zero_disables_the_clock() {
        let mut budget = budget(0, None, None);
        assert!(!budget.deadline_expired(|| panic!("clock disabled")));
        budget.limits.deadline_ms = 5;
        assert!(!budget.deadline_expired(|| 4));
        assert!(budget.deadline_expired(|| 5));
    }
}
