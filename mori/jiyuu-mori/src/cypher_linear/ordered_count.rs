use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ffi::c_long;

use super::{
    ExecutionBudget, ExecutionError, ORDERED_COUNT_GROUP_LIMIT, OrderKey,
    count_group_key, is_native_sort_number, native_sort_integer,
};

const MAX_PASSES: usize = 8;

pub(super) struct OrderedCount {
    pub(super) column: usize,
    max_groups: usize,
    limit: usize,
    batch_limit: usize,
    after: Option<Vec<u8>>,
    overflow: bool,
    passes: usize,
    ordinal: usize,
    numeric_key: Option<bool>,
    groups: BTreeMap<Vec<u8>, Group>,
    winners: Vec<RankedGroup>,
}

struct Group {
    first: usize,
    count: usize,
}

struct RankedGroup {
    key: Vec<u8>,
    group: Group,
    key_number: i32,
    count_number: i32,
}

impl OrderedCount {
    pub(super) fn new(column: usize, max_groups: usize, limit: usize) -> Self {
        Self {
            column,
            max_groups,
            limit,
            batch_limit: max_groups,
            after: None,
            overflow: false,
            passes: 0,
            ordinal: 0,
            numeric_key: None,
            groups: BTreeMap::new(),
            winners: Vec::new(),
        }
    }

    pub(super) fn record(
        &mut self,
        key: &[u8],
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<(), ExecutionError> {
        let key = count_group_key(key);
        let first = self.ordinal;
        self.ordinal += 1;
        if !key.is_empty() {
            self.numeric_key
                .get_or_insert_with(|| is_native_sort_number(key));
        }
        if self.after.as_deref().is_some_and(|after| key <= after) {
            return Ok(());
        }
        if let Some(group) = self.groups.get_mut(key) {
            group.count += 1;
            return Ok(());
        }
        // Keep the smallest keys above the last batch. The largest retained
        // key can only decrease after the map fills, so any key still in the
        // map at the end was counted from its first occurrence in this scan.
        if self.groups.len() == self.batch_limit {
            if !self.overflow {
                if self.passes + 1 == MAX_PASSES
                    || self.batch_limit == 0
                    || self.limit >= ORDERED_COUNT_GROUP_LIMIT
                {
                    budget.check()?;
                    return Err(ExecutionError::Unsupported);
                }
                budget.begin_ordered_count_replay()?;
                self.overflow = true;
            }
            if self
                .groups
                .last_key_value()
                .is_none_or(|(last, _)| key >= last.as_slice())
            {
                return Ok(());
            }
            self.groups.pop_last();
        }
        self.groups.insert(key.to_vec(), Group { first, count: 1 });
        Ok(())
    }

    pub(super) fn finish_batch(
        &mut self,
        order: &[OrderKey],
        columns: usize,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<Option<Vec<Vec<Vec<u8>>>>, ExecutionError> {
        self.passes += 1;
        if self.overflow {
            self.after =
                self.groups.last_key_value().map(|(key, _)| key.clone());
        }

        // Only the requested window survives a batch. The next batch shares
        // the same group cap with these winners.
        let numeric_key = self.numeric_key.unwrap_or(false);
        self.winners.reserve_exact(self.groups.len());
        for (key, group) in std::mem::take(&mut self.groups) {
            budget.step()?;
            let key_number = if numeric_key {
                native_sort_integer(&key)
            } else {
                0
            };
            let count_number =
                c_long::try_from(group.count).unwrap_or(c_long::MAX) as i32;
            self.winners.push(RankedGroup {
                key,
                group,
                key_number,
                count_number,
            });
        }
        budget.check()?;
        let column = self.column;
        self.winners.sort_unstable_by(|left, right| {
            for key in order {
                let cmp = if key.column != column {
                    left.count_number.cmp(&right.count_number)
                } else if numeric_key {
                    left.key_number.cmp(&right.key_number)
                } else {
                    left.key.cmp(&right.key)
                };
                if cmp != Ordering::Equal {
                    return if key.descending { cmp.reverse() } else { cmp };
                }
            }
            left.group.first.cmp(&right.group.first)
        });
        budget.check()?;
        self.winners.truncate(self.limit);
        if self.overflow {
            self.overflow = false;
            self.ordinal = 0;
            self.batch_limit = self
                .max_groups
                .min(ORDERED_COUNT_GROUP_LIMIT - self.winners.len());
            return Ok(None);
        }

        let mut rows = Vec::with_capacity(self.winners.len());
        for winner in self.winners.drain(..) {
            budget.step()?;
            let mut row =
                vec![winner.group.count.to_string().into_bytes(); columns];
            row[column] = winner.key;
            rows.push(row);
        }
        Ok(Some(rows))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::cypher_linear::{
        DEADLINE, ORDERED_COUNT_FALLBACK_RESERVE_MS, ORDERED_COUNT_REPLAY_MAX_MS,
    };

    #[test]
    fn ordered_count_replay_refuses_on_overflow_in_the_final_pass() {
        let mut counts = OrderedCount::new(0, 1, 1);
        let now = Instant::now();
        let clock = || now;
        let mut budget = ExecutionBudget::new(&clock, &|| false);
        let order = [OrderKey {
            column: 1,
            descending: true,
        }];
        for _ in 1..MAX_PASSES {
            for key in (0..=MAX_PASSES as u8).rev() {
                counts.record(&[b'a' + key], &mut budget).unwrap();
            }
            assert!(
                counts
                    .finish_batch(&order, 2, &mut budget)
                    .unwrap()
                    .is_none()
            );
        }
        let key = b'a' + (MAX_PASSES - 1) as u8;
        counts.record(&[key], &mut budget).unwrap();
        counts.record(&[key], &mut budget).unwrap();
        assert!(matches!(
            counts.record(&[key + 1], &mut budget),
            Err(ExecutionError::Unsupported)
        ));
    }

    #[test]
    fn ordered_count_wide_limit_refuses_on_first_overflow() {
        let mut counts = OrderedCount::new(0, 1, ORDERED_COUNT_GROUP_LIMIT);
        let mut budget = ExecutionBudget::new(&Instant::now, &|| false);
        counts.record(b"first", &mut budget).unwrap();
        counts.record(b"first", &mut budget).unwrap();
        assert!(matches!(
            counts.record(b"second", &mut budget),
            Err(ExecutionError::Unsupported)
        ));
        assert!(budget.ordered_count_replay.is_none());
    }

    #[test]
    fn ordered_count_replay_keeps_time_for_fallback() {
        let start = Instant::now();
        let now = Cell::new(start);
        let clock = || now.get();
        let cancelled = Cell::new(false);
        let is_cancelled = || cancelled.get();
        let mut budget = ExecutionBudget::new(&clock, &is_cancelled);
        budget.begin_ordered_count_replay().unwrap();
        now.set(start + Duration::from_millis(ORDERED_COUNT_REPLAY_MAX_MS));
        assert!(matches!(budget.check(), Err(ExecutionError::Unsupported)));
        cancelled.set(true);
        assert!(matches!(budget.check(), Err(ExecutionError::Cancelled)));
        cancelled.set(false);
        now.set(start + DEADLINE);
        assert!(matches!(budget.check(), Err(ExecutionError::Deadline)));

        now.set(start);
        let mut budget = ExecutionBudget::new(&clock, &is_cancelled);
        now.set(
            start + DEADLINE
                - Duration::from_millis(ORDERED_COUNT_FALLBACK_RESERVE_MS),
        );
        assert!(matches!(
            budget.begin_ordered_count_replay(),
            Err(ExecutionError::Unsupported)
        ));
    }

    #[test]
    fn ordered_count_replay_work_limit_is_cumulative() {
        let now = Instant::now();
        let clock = || now;
        let mut budget = ExecutionBudget::new(&clock, &|| false);
        budget.begin_ordered_count_replay().unwrap();
        budget
            .ordered_count_replay
            .as_mut()
            .unwrap()
            .remaining_steps = 2;
        budget.step().unwrap();
        budget.begin_ordered_count_replay().unwrap();
        assert!(matches!(budget.step(), Err(ExecutionError::Unsupported)));
    }
}
