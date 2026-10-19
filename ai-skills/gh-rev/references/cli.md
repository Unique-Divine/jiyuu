# The gh-rev CLI

Command `gh-rev` is the executable used by this skill. It maintains a local
Markdown review ledger, selects immutable review snapshots, and tracks finding
status. It does not judge code or generate review findings. Use the
`gh-pr-review` skill for that work.

## Discover the installed tool

```bash
command -v gh-rev
gh-rev --help
gh-rev --version
gh-rev paths
```

The installed help is authoritative for syntax. Inspect the relevant
subcommand with `gh-rev open --help`, for example. Command `paths` reports the
ledger root and state file. The default root is `~/gh`; environment variable
`GH_REV_HOME` or option `--home` overrides it. Command `--version` reports
package and Git provenance.

The source package lives at `$HOME/ki/boku/jiyuu/gh-rev`; its `README.md`
documents the full command and recovery contract. On this workstation, install
or update through dotfiles:

```bash
just --justfile "$DOTFILES/justfile" gh-rev-install
```

Install only when the task calls for it. An older binary is not a reason to
change the review target or silently lose ledger history.

## Command purposes

| Command | Purpose |
| --- | --- |
| `open` | Select a target and save its base/head snapshot for a review |
| `next` | Allocate the next revision template for the selected authority |
| `status` | Read review counts, recommendations, and target paths |
| `status todo` | List unresolved finding markers |
| `context` | Print or create the target's human-authored context file |
| `sync` | Repair counters and reconcile branch histories with open PRs |
| `register` | Explicitly register a checkout for migration or offline use |

Commands return structured output. Read the returned paths and commit SHAs
instead of constructing ledger paths or guessing revision numbers. Use
`status` for inspection; `open` records a pending review snapshot and `next`
creates a file. Use `context` only when the user requests durable context work.

## Select the review authority

Run from the intended worktree, or use `-C` to select it:

```bash
gh-rev -C /path/to/worktree open
gh-rev -C /path/to/worktree next --label initial
gh-rev -C /path/to/worktree status todo
```

Worktree discovery makes prior registration optional. Commands `open`, `next`,
and `context` discover a unique matching open PR and adopt or reuse its ledger;
without a matching PR, they use branch history. Ambiguous matches require an
explicit PR selector. Use explicit selectors when the request establishes the
authority rather than relying on discovery:

```bash
gh-rev open owner__repo --pr 1153
gh-rev next owner__repo --pr 1153 --label initial
gh-rev status todo owner__repo --pr 1153

gh-rev open owner__repo --branch feature/cache --base main --head local
gh-rev next owner__repo --branch feature/cache --head local --label initial
```

PR mode reviews the published GitHub head. Branch mode reviews the exact local
branch; `--head local` acknowledges local intent when an adopted PR has
diverged. Do not combine `--pr` with `--head local`. Inspect the returned
authority and base/head SHAs before reviewing, and use the same selectors for
allocation. If `next` reports changed commits, reopen and review the new range.

GitHub discovery uses the authenticated `gh` CLI. PR adoption requires an exact
local head branch in the matching repository; fork PR adoption is unsupported.
If a prerequisite is missing, distinguish reviewing the available evidence
from successfully recording it in the ledger.

Command `sync` supports bulk adoption and recovery, but is not required before
ordinary review creation. A busy-ledger error means another writer owns the
lock. Retry after it exits; do not bypass the lock. After another failure,
inspect state before retrying because local repairs may already be persisted.

Writing a revision submits it to the local ledger. Publishing a GitHub review
or comment is a separate action requiring the user's request.
