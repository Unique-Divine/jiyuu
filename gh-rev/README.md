# gh-rev

`gh-rev` maintains a persistent local Markdown ledger for branch and pull
request reviews. Ledger state lives under `~/gh/` by default, outside reviewed
repositories.

## Command contract

Run directly from the current worktree (or a discovered checkout with `-C`):

```bash
cd /path/to/worktree
gh-rev open
gh-rev next --label follow-up
gh-rev status todo
gh-rev sync
```

When the current branch has exactly one open pull request in the checkout's
repository, `open`, `next`, and `context` adopt or reuse that PR's canonical
`pr-N` history automatically. No `register`, `sync`, or `--pr` selector is
needed. With no matching PR, the commands continue in branch history; an
ambiguous match fails and asks for an explicit `--pr` selection.

Use explicit selectors when discovery is not desired:

```bash
gh-rev open owner__repo --branch feature/name --base main
gh-rev next owner__repo --branch feature/name --label follow-up
gh-rev context owner__repo --pr 1153
```

Branch and base names resolve only through `refs/heads/<name>^{commit}`. A tag
with the same name cannot select the review range. `open` saves the selected
base/head snapshot. `next` refreshes the same authority and refuses allocation
if either commit moved. A direct `next` performs selection and allocation while
holding the workspace lock.

`-C <path>` discovers another worktree.

Use `sync` to repair counters, recover interrupted adoption, and reconcile
existing branch histories in bulk. It is not required before ordinary review
creation:

```bash
gh-rev sync owner__repo
gh-rev open owner__repo --pr 1153
gh-rev next owner__repo --pr 1153
```

GitHub metadata is read through the authenticated `gh` executable. Adoption
requires an open PR whose head owner and repository match the checkout and
whose exact local head branch exists. Targeted `--pr` operations remain
available for an explicit published-PR review. Fork PRs are rejected.

`register` remains available for migration and explicit offline workflows, but
automatic discovery makes it optional for normal command usage.

`sync` snapshots repository identity under a short lock, releases it before
invoking `gh`, then reacquires it and rereads current state before any ledger
mutation. It never holds the ledger lock across network I/O. If another
operation still owns the lock after a bounded retry, `sync` exits with a
`ledger is busy` error rather than waiting indefinitely or bypassing the
writer. Targeted `open --pr`, `next --pr`, and `context --pr` use the same
snapshot/fetch/reacquire boundary.

Bulk `sync` only considers branches that already own targets. Its JSON separates
`recovered` interrupted adoptions; `adopted`, `merged`, and `unchanged` PRs;
`skipped_zero` branches;
`skipped_ambiguous` branch-to-PR lists; and `local_repaired` counter changes.
Merged entries include each old PR review number mapped to its appended number.
Zero and ambiguous matches are structured skips rather than fatal errors, so
all local counter repairs are persisted.

After adoption, the branch owns the mutable target in `state.toml`; the PR
number is only an alias to that branch. Branch and PR filters therefore select
the same ledger, and unfiltered status counts it once:

```bash
gh-rev status owner__repo --branch feature/name
gh-rev status owner__repo --pr 1153
gh-rev status todo owner__repo
```

State written by the earlier prototype may contain a full target beneath a PR
key. Targeted PR resolution and `sync` preserve that target and merge its
`pr-N` history into the branch-owned model before writing the alias form.

The GitHub PR head is authoritative for PR reviews. The exact local branch is
authoritative for branch reviews. If those heads diverge, PR commands continue
and report the selected `head_authority`, local branch SHA, optional
`origin/<branch>` SHA, PR SHA, and divergence. Branch `open` and `next` require
explicit local intent:

```bash
gh-rev open owner__repo --branch feature/name --head local
gh-rev next owner__repo --branch feature/name --head local
```

Option `--head local` is invalid with `--pr`; branch reviews otherwise always
use the exact local branch.

## PR adoption and recovery

New branch ledgers use percent-encoded `br-v2-*` directories. Existing
persisted directories remain valid. A pre-registry legacy `br-*` directory is
adopted only when every review header names the exact branch.

PR adoption makes `pr-N` canonical. A branch-only ledger is renamed. When both
directories exist, branch review numbers survive and PR reviews are appended
in numeric order after the highest branch number. Their YAML `review:` fields
are rewritten, all other target evidence remains intact, identical context is
kept once, and differing contexts are combined under source headings.
Repeated synchronization is idempotent.

The merge is prepared in a staging directory before either source changes.
During replacement, hidden `.adopt-pr-*-(branch|pr)-backup` directories retain
both originals until `state.toml` is atomically written. Because the lock is an
OS advisory lock, process exit releases it even though `.gh-rev.lock` remains
on disk.

The next `sync` recovers a recognized interrupted adoption while holding that
lock. It discards abandoned staging when both source histories remain
authoritative, rolls back backups when persisted state still names the old
layout, or removes backups when persisted state already names `pr-N`. Recovery
is limited to PRs known through current aliases or fetched GitHub metadata. An
unknown or structurally ambiguous backup is preserved and reported instead of
being guessed at. Successful commands remove only backups created for PRs
persisted by that command.

`sync` always repairs local counters first. If `gh` is unavailable or GitHub
fails, the repaired local state is still written, stderr reports
`github: unavailable: <reason>`, and the command exits nonzero. This is
intentional partial success; rerun `sync` after restoring GitHub access.

## Development

List the supported commands and run all checks:

```bash
just --list
just check
```

Command `just check` runs formatting verification, Clippy with warnings denied,
and all Rust tests.

## Build provenance

Command `gh-rev --version` prints package and Git provenance as JSON:

```json
{
  "version": "0.1.0",
  "commit": "<git-sha>",
  "dirty": false
}
```

Field `dirty` is `null` when Git metadata was unavailable at build time. The
dirty check is scoped to this `gh-rev` package rather than unrelated changes in
the surrounding `jiyuu` repository.

## Install or update

Install the verified release binary at the path used by this workstation:

```bash
just check
just install-local
```

Override the destination when needed:

```bash
just install-local "$HOME/bin/gh-rev"
```

The install recipe builds with the lockfile, replaces the destination only
after a successful build, and prints the installed binary's provenance.
