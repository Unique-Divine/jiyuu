# Read and work existing revisions

Use this workflow when the goal is to understand review status, implement
requested changes, or resolve findings. This is a primary `gh-rev` use case,
not an afterthought to review creation.

## Example: report review status

For "Are there reviews for this PR?" or "What remains unresolved?":

1. Resolve the target with `gh-rev status`, using `--pr` for published PR state
   or `--branch` for local branch state. Unlike `open`, `status` does not create
   a pending review snapshot.
2. Read `context.md` and every `rev-*.md` beneath the returned `target_path`,
   not only the newest file.
3. Query exact unresolved markers:

   ```bash
   gh-rev status todo owner__repo --pr 1153
   # or: gh-rev status todo owner__repo --branch feature/cache
   ```

4. Summarize review count, recommendations, and unresolved findings. Do not
   infer approval merely from an empty current TODO list; resolved findings and
   each revision's recommendation remain part of the history.

## Example: implement requested changes

For "Take the reviews, fix the requested changes, and document the result":

1. Read repository instructions, the issue or PR intent, `context.md`, every
   revision, and `status todo`. Group duplicate or dependent findings before
   editing.
2. Work in a local checkout whose branch and SHA correspond to the selected
   target. If starting from an online-only PR, use the repository's normal
   checkout/worktree flow to materialize it before editing.
3. Inspect the cited code and verify each finding rather than accepting it
   mechanically. Implement the smallest coherent fix that satisfies the
   underlying concern and repository conventions.
4. Run focused tests for every changed behavior, followed by the repository's
   proportionate type, lint, build, or broader test checks.
5. Re-read the resulting diff and the originating finding. Only after evidence
   shows it is handled, change its exact marker from `- [ ] rev:` to
   `- [x] rev:` in the original `rev-N.md` and add an indented resolution note
   naming the fix and verification.
6. If the finding is invalid or intentionally declined, resolve it with a note
   containing the concrete evidence and decision. Resolution means the concern
   has been considered and closed, not necessarily that its proposed edit was
   accepted.
7. Rerun `gh-rev status todo` and report anything still unresolved.

Do not allocate a new revision just to document fixes to old findings. The
originating revision is the durable task record. Allocate a new revision only
when someone performs another independent review of the resulting code.

## Boundaries

- Editing source files requires the user's request to implement or change code;
  a request to inspect or explain reviews is read-only.
- Checking a finding does not publish to GitHub or imply GitHub approval.
- Preserve review history. Never delete a finding because it was fixed,
  disagreed with, superseded, or inconvenient.
- Use only unresolved and resolved states. Put nuance in the resolution note.
