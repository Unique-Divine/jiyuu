# Example Deal Brief

Use the [synthetic responses](atlas-fixture.json) in place of the live API.
The sample data uses placeholder deal, transaction, and message identifiers.
The exercise needs no account or credentials.

## Prompt

> Use the `sai-deals` skill to brief me on `EXAMPLE-ATLAS` through November
> 22, 2026 UTC. Did the trader meet the withdrawal milestone? Show what we
> agreed, what the saved evaluation covers, what we issued, and what the
> communication evidence establishes. Cite IDs and dates. Give me one next
> step for review.

Start with the fixture's `views.EXAMPLE-ATLAS` entry. Read the first
`commsPages` entry at key `""`; then follow its cursor for the second page.
Use each row's returned reader ID to find the corresponding entry in
`commsDetails`. The same navigation applies to the commands in
[Brief One Deal](../workflows/deal-review.md).

## Expected Brief

- Deal `deal-example-atlas` records a 250.00 USDC trading-credit grant
  under term `term-atlas-volume`. Term `term-atlas-unlock` sets a
  10,000.00 USD volume threshold for release.
- Evaluation `eval-atlas-volume` records 8,100.00 USD at the 2026-11-20
  00:00 UTC cutoff. It leaves the rest of the requested period open, so
  the records do not establish whether the trader crossed the threshold
  through November 22.
- Payment `payment-atlas-1` records execution of the 250.00 USDC
  trading-credit grant on November 18 at 16:00 UTC. Action
  `action-atlas-setup` records setup on November 18 at 15:42 UTC. The
  listed payment form is trading credit. The fixture has no withdrawal
  release payment.
- Communication `completed_note:comms-atlas-paid` is on the second page.
  Its completed attempt records delivery at 16:12 UTC on November 18,
  with message ID `fixture-atlas-message-1` and a link to the payment.
  Note `comms-atlas-note` has an entry date of November 19 and no
  verified delivery time. Workflow `comms-atlas-check` has an
  uncertain attempt on November 22. Treat the latter two as separate
  from the verified grant confirmation.
- Next step. Check a current measurement with sufficient coverage and
  ask the operator to verify any recorded release before confirming
  withdrawability.

## Two Variations

- Resolve the fixture wallet instead of the referral code. It matches
  `deal-example-atlas` and `deal-example-boreal`. Ask which deal to brief
  before reading either deal's communications.
- Use `keeperCheck` to compare the exact lifetime counters at block 100.
  Keeper reports 8,000.00 USD against the contract's 8,100.00 USD using
  six base-unit decimals. The counters differ at the same cutoff and
  the fixture lacks an indexing watermark. Report that the independent
  projection does not corroborate the saved figure, while leaving the
  saved evaluation and withdrawal outcome as separate findings.
