# Fix gh-rev review state reconciliation

Make `gh-rev` recover its allocation counter from existing review artifacts so a lost or stale target-state entry cannot block subsequent reviews.

- Closes https://github.com/Unique-Divine/jiyuu/issues/26

## Rationale

Review artifacts are durable evidence that a number was already allocated, while `state.toml` can be missing a target mapping or retain an older counter. Treating state as the sole allocation source caused `next` to retry an existing filename indefinitely and required manual TOML edits. The repair preserves existing target metadata and only advances an unsafe counter; it never reuses an artifact number.

## Key Changes

1. Add the `gh-rev` local review-ledger CLI with persistent, lock-protected state and review artifacts.
1. Reconcile target allocation state before `open`, `next`, and `context`, then persist any repaired counter through the existing atomic state writer.
1. Accept exact positive numeric review names, preserve numbering gaps, and treat occupied matching paths conservatively so allocation cannot collide with an existing directory.
1. Add end-to-end regressions for missing state, stale state, gaps, unrelated names, and both recovery paths.

## 1 - Artifact-backed allocation recovery

```text
state.next_review_number  ─┐
                            ├─ max(...) -> next allocation
highest rev-<number>.md + 1 ─┘
```

When a target directory contains valid review artifacts, the highest artifact number is authoritative evidence of the lower bound for future allocation. A missing branch target is reconstructed using the canonical branch directory, then reconciled before output or creation. `next` therefore allocates `rev-3.md` after `rev-1.md` and `rev-2.md`, whether recovery occurs through `open` first or directly through `next`.

The counter only moves forward. A gap such as `rev-1.md` plus `rev-3.md` allocates `rev-4.md`, preserving review-history ordering rather than filling an old gap.

## 2 - Valid artifact boundary and operator feedback

Only exact positive numeric review names participate in reconciliation; files such as `rev-x.md`, `rev-2.md.bak`, and `review-99.md` are ignored. A matching occupied directory is treated as unavailable and advances allocation, preventing a later write collision. When repair advances state, `gh-rev` reports the old and new allocation values on stderr. A `u64`-maximum artifact produces an explicit operator-repair error instead of wrapping the counter or overwriting an artifact.
