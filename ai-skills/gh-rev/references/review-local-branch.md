# Review a local branch

Use this workflow when the review target is an exact local branch, whether or
not a pull request exists yet.

## Example: draft branch without a PR

For a request such as "Use gh-rev to review my `feature/cache` worktree":

1. Identify the checkout, repository slug (`owner__repo`), branch, and intended
   base from Git and the user's context. Do not assume `main` if repository
   instructions or branch metadata establish another base.
2. Register the checkout if `gh-rev` reports the repository as unknown:

   ```bash
   gh-rev register /absolute/path/to/checkout
   ```

3. Resolve the review range and ledger:

   ```bash
   gh-rev open owner__repo --branch feature/cache --base main
   ```

   If the branch was adopted by a PR and the current CLI requires explicit
   local authority, add `--head local`.
4. Read the returned `context_path` and every prior review. Read the linked
   issue or durable design notes before judging whether the implementation
   matches its intent.
5. Apply `gh-pr-review` to the exact `base_sha...head_sha` reported by
   `gh-rev`, including surrounding code and focused tests. Do not review an
   ambient worktree `HEAD` when it differs from the named branch.
6. When the findings are final, allocate the matching revision:

   ```bash
   gh-rev next owner__repo --branch feature/cache --label initial
   ```

   Add `--head local` if it was required by `open`.
7. Fill the returned `review_path`. Remove the placeholder finding, write each
   real finding with the exact marker described in `SKILL.md`, and include the
   `gh-pr-review` recommendation.
8. Verify the artifact and scanner result:

   ```bash
   gh-rev status owner__repo --branch feature/cache
   gh-rev status todo owner__repo --branch feature/cache
   ```

If `next` refuses because the base or head moved after `open`, do not force the
old analysis into a new snapshot. Re-open the branch, inspect the new diff, and
adjust the review before allocating.

## Example: local work beyond a published PR

When a branch has a PR but the user specifically wants unpublished local work
reviewed, use branch mode and explicit local authority. Mention the divergence
in `## Scope`. Use PR mode instead if the user wants the code currently visible
to GitHub reviewers.

Do not create a PR merely to use the ledger. Branch histories are intentionally
useful for draft and private work and can be adopted by `gh-rev sync` later.
