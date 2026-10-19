# Review a pull request

Use this workflow for the PR state published on GitHub. A source checkout is
helpful for deep inspection and tests, but it is not a prerequisite for reading
the PR or producing a review.

## Example: online PR

For a request such as "Review PR #1153 with gh-rev":

1. Resolve the repository, PR number, issue, PR description, prior GitHub
   comments, base/head refs, and immutable base/head SHAs with `gh`.
2. Inspect the installed CLI as described in [cli.md](cli.md).
3. Use a suitable repository checkout, discovered from the command directory
   or selected with `-C`, and open the PR ledger:

   ```bash
   gh-rev open owner__repo --pr 1153
   ```

   Read the returned context and all prior revisions. Opening the PR may adopt
   branch history into its canonical ledger; do not duplicate adopted reviews.
   Use `sync` for bulk reconciliation or interrupted-adoption recovery, not as
   a prerequisite for each review.
4. Use the `gh-pr-review` skill to review the GitHub PR head against its GitHub
   base. Use `gh pr diff`, surrounding source from GitHub, and available
   repository context. Materialize or update a local checkout when tests,
   generated files, cross-file searches, or implementation work make that
   useful.
5. Confirm the `gh-rev open` head SHA matches the PR head reviewed. Then
   allocate and fill the revision:

   ```bash
   gh-rev next owner__repo --pr 1153 --label independent
   ```

6. Verify the PR ledger:

   ```bash
   gh-rev status owner__repo --pr 1153
   gh-rev status todo owner__repo --pr 1153
   ```

PR mode uses the GitHub PR head as authority. Do not use `--head local` with
`--pr`, and do not silently review extra local commits.

## Local branch prerequisite

The current PR adoption model requires an exact local branch matching the
GitHub head branch, even though the GitHub PR head SHA remains authoritative
for the review. In that case:

- Still complete the requested online review from immutable GitHub base/head
  evidence; lack of a source checkout is not a reason to skip review.
- If a matching local checkout already exists, use it only after confirming its
  exact branch SHA matches the GitHub PR head.
- If the user also asks for implementation or local verification, materialize
  the branch through the repository's normal checkout/worktree workflow.
- Otherwise, report that the review could not yet be recorded in `gh-rev` and
  identify the missing local branch prerequisite. Do not fabricate a branch
  ledger, substitute a different SHA, or claim a revision was submitted.

## Another independent review

Read every existing revision and GitHub review first, but independently inspect
the full change. Avoid repeating an existing unresolved finding unless new
evidence materially changes it. Allocate a new revision because this is a new
review pass, even when it concludes with approval.
