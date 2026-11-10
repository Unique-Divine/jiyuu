---
name: gh-labels
description: >-
  Reference for what GitHub issue/PR labels mean in this workspace (S-triage,
  S-needs-scope, S-impl, prio-p0-critical, A-ext-dev-tools, A-perf, A-ux, and
  others under My Labels). Use when the user asks what a label means, which
  label to apply, or how to add missing labels to a repo.
  Also use for repository label conventions, path-based PR labeling, merged
  PR label inheritance, historical backfills, and bootstrapping missing labels.
---

# GitHub labels

Reference for **what labels mean**, then how to add them to a repo that does not
have them yet.

## Repository Conventions

Read the repository's root `GITHUB.md` when present, then its `AGENTS.md` and
labeler configuration for local rules. The shared vocabulary below supplies
defaults; repository instructions define local zone names and path mappings.
Reuse existing repository automation. Plain instructions and path mappings
usually suffice for a small label set.

Use stable area names across repositories when they describe the same work.
For example, `A-ai` can group public MCP development with private AI planning
in GitHub Projects. Sharing a label does not authorize copying private content
or changing another repository.

Path automation can synchronize labels listed in its mapping, including
overlapping parent and child zones. Do not remove manually assigned status,
priority, mission, or other labels outside that mapping.

Where a repository enables merged PR inheritance, pass classification labels
matching `^[A-Z]-.+$` except `S-` to its same-repository closing issues. This
includes `A-`, `Z-`, `M-`, and future uppercase families. Add missing labels;
never replace the issue's full label set. Keep cross-repository inheritance
disabled unless separately designed and authorized. Jiyuu's root `GITHUB.md`
documents its typed dry-run/replay command and CI behavior.

When first enabling inheritance, verify it with a small subsequent PR after
the workflow reaches the default branch. The bootstrap merge does not prove
the workflow was active on its base. Use a useful scoped change and a
same-repository closing issue. Let path automation classify the PR, then check
that the issue gains eligible labels while retaining its status, priority,
and existing classifications. Replay the planner after merge to confirm no
additions remain.

For a historical backfill, inspect all changed-file pages and issue scope,
account for renamed paths, and add only supported classifications. Do not
infer an issue's complete label set from a mixed PR. Preserve statuses and
priorities, report uncertain items, and keep private evidence out of public
issues, PRs, and logs.

## My labels

Prefix convention:

- `S-*` labels describe issue status.
- `prio-p*` labels describe issue priority.
- `A-*` labels describe product or engineering areas.
- `M-*` labels describe milestones or missions within a repo.

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

- `A-ai`: (#eab6cb) | Area: AI tooling, MCP servers, agent skills, and agent workflow tools
  - Includes MCP servers, agent skills, and tools for agent workflows.
  - Repositories define their own path mappings. A Telegram CLI is not part
    of an MCP zone solely because both use Telegram.
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
- `A-security`: (#eab6cb) | Area: Security hardening, advisories, and upstream-sourced fixes
  - Use for advisories, vulnerability fixes, hardening, and tickets sourced
    from upstream repos by agent skill `upstream-watch`. Track mainnet-risk
    findings in private NibiruChain/sai-perps issues.

### Mission labels

- `M-launch-req`: (#04d0e4) | Mission: Required before launch.
  - Adopted in DSN Braid. Combine with priority to distinguish launch scope
    from work order; absence of this label does not imply a status.

### Zone labels

- `Z-*` labels name a code zone within a single repo, such as one
  subdirectory. Each repo defines its own; check its root `GITHUB.md`, labeler
  configuration, and `gh label list`. Parent and child zones may overlap.

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

gh label create "A-ai" \
  --description "Area: AI tooling, MCP servers, agent skills, and agent workflow tools" \
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

gh label create "A-security" \
  --description "Area: Security hardening, advisories, and upstream-sourced fixes" \
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
