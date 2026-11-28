# Implement changes based on review submissions

Use this workflow when the user asks to implement, address, or automatically
work through submitted review feedback. Review findings are evidence-bearing
inputs to implementation, not commands whose proposed solutions must be
accepted literally.

## Establish the review authority

1. Resolve the requested branch or PR with `gh-rev status` and `status todo`.
2. Read `context.md`, every `rev-*.md`, the relevant issue or specification,
   and repository instructions.
3. Work in a checkout whose branch and SHA correspond to the selected target.
   Do not silently mix pre-existing unrelated local changes into work requested
   for a published PR.
4. Group duplicate or dependent findings before editing. Preserve the
   originating revision and exact finding text.

## Honor the requested control level

Interpret these common instructions as explicit boundaries:

- **Report only:** assess and explain findings without editing code or changing
  review markers.
- **Safe fixes:** implement only findings that meet the automatic-fix criteria
  below, and pause on meaningful decisions. Use this as the default when the
  user asks to address feedback without granting broader discretion.
- **Full implementation:** attempt every verified finding, but still ask before
  making decisions reserved for the user.
- **No scope expansion:** address only the submitted findings and defects
  directly exposed by their fixes.
- **Durable independent review:** allocate and submit a new `gh-rev` revision
  for the additional review pass. Otherwise, any separate verification pass is
  ephemeral.

The user may combine or restate these boundaries in ordinary language. Follow
their stated level of control rather than requiring exact keywords.

## Separate the concern from the proposed solution

For each finding, identify:

- **Concern:** the claimed defect, risk, or missing behavior.
- **Evidence:** the cited code, runtime behavior, failing case, or invariant.
- **Impact:** what becomes incorrect, unsafe, incompatible, or unreliable.
- **Proposed solution:** the reviewer's suggested implementation, if any.

Treat the concern as review input and the proposed solution as
non-authoritative. Verify the concern independently against the code and
intended behavior. A valid concern may need a different implementation than
the reviewer suggested; an invalid concern may be resolved with evidence and no
source change.

Do not mark a finding resolved merely because its suggested edit was applied.
The relevant behavior or invariant must be verified.

## Decide what may be addressed automatically

Automatically implement a finding when all of the following are true:

- Concrete evidence supports the concern.
- The intended behavior is already established.
- The fix is local, high-confidence, and within the user's requested scope.
- The fix preserves settled architecture, product semantics, and compatibility.
- Focused verification can demonstrate the correction.

Pause and ask the user when choosing a resolution would materially affect:

- architecture or component boundaries;
- product behavior or user-visible semantics;
- public APIs, schemas, migrations, or compatibility policy;
- security, custody, accounting, or external state;
- a settled decision in an issue, specification, or review context;
- the implementation scope or delivery sequence.

When pausing, present the verified concern separately from the reviewer's
proposed solution. Explain the meaningful options and tradeoffs without
assuming the reviewer's recommendation is preferred.

It is acceptable to disagree with a finding. Resolve it only after recording
the concrete evidence and decision in its originating revision.

## Implement and verify

1. Make the smallest coherent change that addresses the verified concern.
2. Add focused regression coverage for the failure or invariant when
   deterministic coverage is practical.
3. Run the smallest relevant checks, then proportionate type, lint, build, or
   broader tests for the changed boundary.
4. Re-read the resulting diff and the original finding. Confirm the concern,
   not merely the suggested edit, is handled.
5. Change the originating marker from `- [ ] rev:` to `- [x] rev:` only after
   that evidence exists. Add an indented `Resolved:` note naming the decision,
   implementation, and verification.

Do not allocate a new revision to document fixes to existing feedback. The
originating finding remains the durable task record.

## Use a separate verification pass for non-obvious changes

For changes involving concurrency, persistence, migrations, security,
recovery, filesystem replacement, or other non-obvious failure modes, ask a
separate read-only reviewer to inspect the implemented diff before handoff when
an independent-review capability is available.

Give that reviewer:

- the intended behavior and settled constraints;
- the exact diff and relevant surrounding code;
- the original findings and which concerns were accepted or declined;
- the verification already performed.

Ask for actionable P0-P2 correctness findings with concrete evidence. Require
the reviewer to distinguish the observed problem from any suggested
remediation. The reviewer must not edit files.

Verify any returned concern before changing code. Automatically address only
findings that satisfy the safe criteria above; ask the user about meaningful
design choices. After accepted corrections, use the same reviewer to verify its
original findings against the updated diff. This verification should not
expand scope unless it identifies another material defect.

This read-only pass is an implementation guardrail, not automatically a new
`gh-rev` submission or GitHub review. Allocate a new durable revision only when
the user asks for another independent review pass, not merely because a
reviewer verified fixes during implementation.

## Finish the feedback work

Rerun `gh-rev status todo` and report:

- findings resolved with code and verification;
- findings resolved by evidence-backed disagreement;
- findings still open or awaiting a user decision;
- any independent verification performed and whether it was durable or
  ephemeral.

Checking a finding does not publish to GitHub or imply GitHub approval. Never
delete review history because a finding was fixed, declined, superseded, or
inconvenient.
