---
name: gh-labels
description: >-
  Reference for what GitHub issue/PR labels mean in this workspace (S-triage,
  S-needs-scope, S-impl, prio-p0-critical, A-dev-tools, and others under My
  Labels). Use when the user asks what a label means, which label to apply, or
  how to add missing labels to a repo with `gh label create --force`.
  Secondary: bootstrap labels on a new repo that does not have them yet.
---

# GitHub labels

Reference for **what labels mean**, then how to add them to a repo that does not
have them yet.

## My Labels

Prefix convention:
- `S-*` labels describe issue status.
- `prio-p*` labels describe issue priority.
- `A-*` labels describe product or engineering areas.

#### Status (S) Labels
- `S-triage`: (#F9D0C4) | Status: This issue is waiting on initial triage. More Info: https://tinyurl.com/25uty9w5
- `S-needs-scope`: (#f08583) | Needs someone to work further on the design for the feature or fix. NOT YET accepted.
- `S-impl`: (#04d0e4) | Status: Implementation-ready. Clear and fully specificed
- `S-blocked`: (#f08583) | Status: 🧱 Blocked by an external dependency or unresolved decision.
- `S-someday-maybe`: (#f08583) | Status: 🤔 Intentionally paused. Currently on hold or out of scope.

#### Priority Labels
- `prio-p0-critical`: (#aaeaec) | Priority: Critical or super urgent
- `prio-p1-high`: (#aaeaec) | Priority: High
- `prio-p2-normal`: (#aaeaec) | Priority: Normal
- `prio-p3-low`: (#aaeaec) | Priority: Low. Please focus on p2 or higher for now.

#### Area (A) Labels
- `A-dev-tools`: (#eab6cb) | Area: Tools and changes for external developers and builders. NOT end users
  - Use for issues that improve SDKs, CLIs, examples, docs, codegen, APIs, or
  other tooling for external devs. NOT repo contributors. 
  - Do not use for purely internal CI or repo automation.
  - You should rarely be using this label because we don't work on external
  dev tooling often. Most tool development is internal for employees and team
  members in the org.

When the user adds a standard label, append a bullet here with the same format:
`` `<name>`: (#<RRGGBB>) | <description> `` - Keep their description text verbatim.

## Add labels to a repo

Use this when a repo is missing labels from **My Labels**, or has stale
name/color/description.

1. **Pick the repo** — default: current git checkout. Otherwise `-R OWNER/REPO`.
2. **See what exists** — `gh label list` (add `-R` if needed).
3. **Create only what is missing** — one `gh label create` per label from **My
   Labels**, using the exact description and color from the bullet.
4. **If the label already exists but is wrong** — rerun the same command with
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

gh label create "A-dev-tools" \
  --description "Area: Tools and changes for external developers and builders. NOT end users" \
  --color eab6cb \
  --force
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
