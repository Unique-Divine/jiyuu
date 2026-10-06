---
name: gh-labels
description: >-
  Reference for what GitHub issue/PR labels mean in this workspace (S-triage,
  S-needs-scope, S-impl, prio-p0-critical, A-ext-dev-tools, A-perf, A-ux, and
  others under My Labels). Use when the user asks what a label means, which
  label to apply, or how to add missing labels to a repo.
  Secondary: bootstrap labels on a new repo that does not have them yet.
---

# GitHub labels

Reference for **what labels mean**, then how to add them to a repo that does not
have them yet.

## My labels

Prefix convention:

- `S-*` labels describe issue status.
- `prio-p*` labels describe issue priority.
- `A-*` labels describe product or engineering areas.

### Status labels

- `S-triage`: (#F9D0C4) | Status: This issue is waiting on initial triage. More Info: https://tinyurl.com/25uty9w5
- `S-needs-scope`: (#f08583) | Needs someone to work further on the design for the feature or fix. NOT YET accepted.
- `S-impl`: (#04d0e4) | Status: Implementation-ready. Clear and fully specificed
- `S-blocked`: (#f08583) | Status: 🧱 Blocked by an external dependency or unresolved decision.
- `S-someday-maybe`: (#f08583) | Status: 🤔 Intentionally paused. Currently on hold or out of scope.

### Priority labels

- `prio-p0-critical`: (#aaeaec) | Priority: Critical or super urgent
- `prio-p1-high`: (#aaeaec) | Priority: High
- `prio-p2-normal`: (#aaeaec) | Priority: Normal
- `prio-p3-low`: (#aaeaec) | Priority: Low. Please focus on p2 or higher for now.

### Area labels

- `A-ext-dev-tools`: (#eab6cb) | Area: Tools built for developers outside the organization
  - Use for external SDKs, libraries, codegen, examples, and developer docs,
    such as the Nibiru TypeScript package `nibijs`.
  - A CLI or API does not qualify just because developers can use it. Check
    whether the work primarily serves external developers rather than internal
    operations or end-user flows.
- `A-perf`: (#eab6cb) | Area: End-user speed, throughput, latency, and work that measures or improves them
  - Includes block time, transaction throughput, caching, benchmarks, and
    observability tied to a performance goal.
- `A-tech-investment`: (#eab6cb) | Area: Internal work that helps the team ship faster
  - Includes CI, E2E tests, automation, AI workflows, team communication, and
    internal tools. Use label `A-perf` when the goal is end-user speed.
- `A-ops`: (#eab6cb) | Area: Operational work such as deployments, governance proposals, and bookkeeping
  - Use for carrying out on-chain deployments and governance actions. Work on
    deployment tooling belongs under label `A-tech-investment`.
- `A-ux`: (#eab6cb) | Area: Solve friction in apps and wallets; improve acquisition, activation, referrals, and retention
  - Use for solutions to friction users encounter in apps, wallets, or other
    flows, and for work geared toward user acquisition or activation, referrals,
    and retention.

When the user adds a standard label, append a bullet here with the same format:
`` `<name>`: (#<RRGGBB>) | <description> `` - Keep their description text verbatim.

## Add labels to a repo

Use this when a repo is missing labels from **My Labels**, or has stale
name/color/description.

1. **Pick the repo** — default: current git checkout. Otherwise `-R OWNER/REPO`.
2. **See what exists** — `gh label list` (add `-R` if needed).
3. **Rename the old area label if present** — rename `A-dev-tools` to
   `A-ext-dev-tools` in place with command `gh label edit --name` so existing issues
   retain the label. Update its description and color at the same time.
4. **Create only what is missing** — one `gh label create` per label from **My
   Labels**, using the exact description and color from the bullet.
5. **If the label already exists but is wrong** — rerun the same command with
   `--force` to update color and description without deleting the label.

Template (fill from the matching **My Labels** bullet):

```bash
gh label create "<name>" \
  --description "<description from My Labels>" \
  --color RRGGBB \
  --force
```

- **Color**: pass the 6-character hex **without** `#` to `gh label create`
  even though **My Labels** shows colors with `#` for editor highlighting.
- **`--force`**: safe when refreshing an existing label to match this reference;
  omit only if you want a hard failure on duplicate names.

### Install commands (copy-paste)

Run from a clone of the target repo, or add `-R OWNER/REPO` to each command.

```bash
gh label create "S-triage" \
  --description "Status: This issue is waiting on initial triage. More Info: https://tinyurl.com/25uty9w5" \
  --color F9D0C4 \
  --force

gh label create "S-needs-scope" \
  --description "Needs someone to work further on the design for the feature or fix. NOT YET accepted." \
  --color f08583 \
  --force

gh label create "S-impl" \
  --description "Status: Implementation-ready. Clear and fully specificed" \
  --color 04d0e4 \
  --force

gh label create "S-blocked" \
  --description "Status: 🧱 Blocked by an external dependency or unresolved decision." \
  --color f08583 \
  --force

gh label create "S-someday-maybe" \
  --description "Status: 🤔 Intentionally paused. Currently on hold or out of scope." \
  --color f08583 \
  --force

gh label create "prio-p2-normal" \
  --description "Priority: Normal" \
  --color aaeaec \
  --force

gh label create "A-ext-dev-tools" \
  --description "Area: Tools built for developers outside the organization" \
  --color eab6cb \
  --force

gh label create "A-perf" \
  --description "Area: End-user speed, throughput, latency, and work that measures or improves them" \
  --color eab6cb \
  --force

gh label create "A-tech-investment" \
  --description "Area: Internal work that helps the team ship faster" \
  --color eab6cb \
  --force

gh label create "A-ops" \
  --description "Area: Operational work such as deployments, governance proposals, and bookkeeping" \
  --color eab6cb \
  --force

gh label create "A-ux" \
  --description "Area: Solve friction in apps and wallets; improve acquisition, activation, referrals, and retention" \
  --color eab6cb \
  --force
```

For a repo that already has `A-dev-tools`, run this before the create commands:

```bash
gh label edit "A-dev-tools" \
  --name "A-ext-dev-tools" \
  --description "Area: Tools built for developers outside the organization" \
  --color eab6cb
```

## Optional: apply a label to an issue or PR

After labels exist on the repo:

```bash
gh issue edit <number> --add-label "S-triage"
gh pr edit <number> --add-label "S-needs-scope"
```

## Other `gh` label commands

```bash
gh label edit "<name>" --description "..." --color RRGGBB
gh label delete "<name>"
```
