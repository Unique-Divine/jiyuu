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
    pub(super) fn new(column: usize, max_groups: usize) -> Self {
        Self {
            column,
            max_groups,
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

    pub(super) fn record(&mut self, key: &[u8]) {
        let key = count_group_key(key);
        let first = self.ordinal;
        self.ordinal += 1;
        if !key.is_empty() {
            self.numeric_key
                .get_or_insert_with(|| is_native_sort_number(key));
        }
        if self.after.as_deref().is_some_and(|after| key <= after) {
            return;
        }
        if let Some(group) = self.groups.get_mut(key) {
            group.count += 1;
            return;
        }
        // Keep the smallest keys above the last batch. The largest retained
        // key can only decrease after the map fills, so any key still in the
        // map at the end was counted from its first occurrence in this scan.
        if self.groups.len() == self.batch_limit {
            self.overflow = true;
            if self
                .groups
                .last_key_value()
                .is_none_or(|(last, _)| key >= last.as_slice())
            {
                return;
            }
            self.groups.pop_last();
        }
        self.groups.insert(key.to_vec(), Group { first, count: 1 });
    }

    pub(super) fn finish_batch(
        &mut self,
        order: &[OrderKey],
        columns: usize,
        limit: usize,
        budget: &mut ExecutionBudget<'_>,
    ) -> Result<Option<Vec<Vec<Vec<u8>>>>, ExecutionError> {
        self.passes += 1;
        if self.overflow {
            budget.check()?;
            if self.passes == MAX_PASSES
                || self.max_groups == 0
                || limit >= ORDERED_COUNT_GROUP_LIMIT
            {
                return Err(ExecutionError::Unsupported);
            }
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
        self.winners.truncate(limit);
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
    use std::time::Instant;

    use super::*;

    #[test]
    fn ordered_count_replay_refuses_after_pass_cap() {
        let mut counts = OrderedCount::new(0, 1);
        let mut budget = ExecutionBudget::new(&Instant::now, &|| false);
        let order = [OrderKey {
            column: 1,
            descending: true,
        }];
        for _ in 1..MAX_PASSES {
            for key in (0..=MAX_PASSES as u8).rev() {
                counts.record(&[b'a' + key]);
            }
            assert!(
                counts
                    .finish_batch(&order, 2, 1, &mut budget)
                    .unwrap()
                    .is_none()
            );
        }
        for key in (0..=MAX_PASSES as u8).rev() {
            counts.record(&[b'a' + key]);
        }
        assert!(matches!(
            counts.finish_batch(&order, 2, 1, &mut budget),
            Err(ExecutionError::Unsupported)
        ));
    }
}
