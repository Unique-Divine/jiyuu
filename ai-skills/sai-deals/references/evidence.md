# Evidence

| Question | Read | What to cite |
| --- | --- | --- |
| Who is this about? | Deal, directory, and matched selectors | Stable deal ID, matching code or wallet, and any lookup errors |
| What was agreed? | Terms in field `signals` | Term ID, condition, period, amount, and payment form |
| What was measured? | Saved evaluations and coverage | Evaluation ID, source, cutoff, and any missing days or measurements |
| What happened? | Payments, completed actions, and execution evidence | Record ID, term link, event date, amount and form, and receipt or other evidence |
| What did the trader hear? | Typed communication records and delivery evidence | Communication ID, linked records, recorded date, and delivery outcome |

A term establishes the agreement. A saved evaluation describes the evidence
available to the evaluator at its cutoff. A recorded zero needs coverage for
that window. A missing evaluation or incomplete history leaves the outcome
unknown for the uncovered scope.

Keep amounts as exact decimal strings. USDC trading credits fund trading and
can be subject to release conditions. A withdrawable Sai deposit is credited
to the Sai balance. A wallet transfer is on-chain delivery to a wallet. Report
the form on each payment and use a specific receipt when verifying execution.

## Communications

Check both direct payment or action links and notes linked to the deal.
Match each event to the selected deal when a note has several deal IDs.

- `SENT` can describe a stored completed note. When `deliveredAt` is null,
  report the note and say no delivery time is verified.
- `PENDING` describes a workflow ready to send. Read the detail and use
  `reviewReason` for `NEEDS_REVIEW`, which can include an uncertain send.
- `CLOSED` describes a workflow dismissed without sending.
- A completed delivery attempt with its message ID, destination, and timestamp
  supports a verified delivery claim. An uncertain attempt leaves delivery
  unresolved. A record's `createdAt` or `updatedAt` describes the record.

Use the reader ID returned by the typed API, and check whether its history or
delivery evidence was truncated. A missing link means there is no directly
linked communication in the queried records. Check deal-level notes before
proposing a follow-up.

## Bounded Keeper Check

When asked to corroborate measured volume, pick one code or wallet and one
interval. Compare the saved evaluation and an independent indexed observation
using the same network, qualifying trades, units, and cutoff. Report both
source cutoffs and the windows each source covers before drawing a conclusion.

For a lifetime volume checkpoint, use the exact indexed base-unit counter and
the contract counter at the same block. A floating-point aggregate is for
display. Report unavailable checkpoints, stale indexing, or mismatched scope
as a gap in corroboration. Use the chain receipt to check a specific
executed transfer. For query details, use the
[Keeper GraphQL reference](https://github.com/Unique-Divine/jiyuu/blob/main/ai-skills/sai/references/graphql.md)
and the [reconciliation guide](https://github.com/Unique-Divine/jiyuu/blob/main/ai-skills/sai/references/reconciliation.md).
