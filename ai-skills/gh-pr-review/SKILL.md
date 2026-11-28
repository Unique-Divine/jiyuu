---
name: gh-pr-review
description: Review GitHub pull requests for correctness, design quality, test coverage, security, performance, and backward compatibility. Use when reviewing PRs, when asked to review code changes, or when the user mentions "review PR", "code review", or "check this PR".
---

# GitHub PR Review Skill

Review pull requests with emphasis on what automated checks usually miss:
correctness, design quality, security, backward compatibility, test adequacy,
maintainability, concurrency risks, and performance regressions.

## Usage Modes

### No Argument

If the user invokes `/gh-pr-review` with no arguments, do not perform a review.
Ask what they want reviewed:

> What would you like me to review?
> - A PR number or URL, for example `/gh-pr-review 12345`
> - A local branch, for example `/gh-pr-review branch`

### Pull Request Mode

The user provides a PR number or URL:

```
/gh-pr-review 12345
/gh-pr-review https://github.com/org/repo/pull/12345
```

For a detailed review with line-specific comments:

```
/gh-pr-review 12345 detailed
```

Use `gh` CLI to fetch PR data:

```bash
# Get PR details
gh pr view <PR_NUMBER> --json title,body,author,baseRefName,headRefName,files,additions,deletions,commits

# Get the diff
gh pr diff <PR_NUMBER>

# Get PR comments and reviews
gh pr view <PR_NUMBER> --json comments,reviews
```

### Local Branch Mode

Review changes in the current branch that are not in `main`:

```
/gh-pr-review branch
/gh-pr-review branch detailed
```

Use git commands to understand branch changes:

```bash
# Get current branch name
git branch --show-current

# Get list of changed files compared to main
git diff --name-only main...HEAD

# Get full diff compared to main
git diff main...HEAD

# Get commit log for the branch
git log main..HEAD --oneline

# Get diff stats
git diff --stat main...HEAD
```

For local branch reviews:
- The Summary should describe what the branch changes accomplish based on commit messages and diff.
- Use the current branch name in the review header instead of a PR number.
- All other review criteria apply the same as PR reviews.

### GitHub Actions Mode

When invoked from a GitHub PR automation, PR metadata may already be injected
into the prompt. Detect this mode by the presence of structured PR context,
PR body, comments, or review comments in the prompt.

The prompt may already contain:
- PR metadata: title, author, branch names, additions/deletions, and file count
- PR body or description
- Comments and review comments with file or line references
- Changed file paths and change types

Use git commands to get the diff and commit history. The base branch name is in
the prompt context, for example `PR Branch: <head> -> <base>` or a `baseBranch`
field.

```bash
# Get the full diff against the base branch
git diff origin/<baseBranch>...HEAD

# Get diff stats
git diff --stat origin/<baseBranch>...HEAD

# Get commit history for this PR
git log origin/<baseBranch>..HEAD --oneline

# If the base branch ref is not available, fetch it first
git fetch origin <baseBranch> --depth=1
```

Do not use `gh` CLI commands in this mode if the action environment only
provides git and injected PR metadata.

## Review Philosophy

2. **Investigate, don't guess** - When uncertain whether a checklist item
   applies, inspect surrounding code or spawn a sub-agent to gather context.
   Treat the stated rationale as a claim to examine. Seek evidence in code,
   history, and documentation that challenges its assumptions. Consult
   external sources when a concrete question warrants it.
3. **Review the design, not just the implementation** - Question hidden state,
   side-channel behavior, unclear ownership, fragile ordering, and missing
   interface contracts. Assess whether the proposed change solves the intended
   problem and whether its tradeoffs make sense for this repository, even when
   the implementation is correct.
5. **Everything reported should matter** - Avoid nits. If it is worth writing
   down, it should affect correctness, maintainability, security, performance,
   compatibility, or reviewability.
6. **Be specific and actionable** - Reference file paths and line numbers.
   Name the function, module, pattern, or API the author should use.
7. **Match the immediate context** - Read how similar behavior is already
   implemented nearby and consult documented conventions. Consider whether a
   departure is intentional and justified, improves the convention, or is an
   unexplained inconsistency. A difference alone is not a finding.
8. **Assume competence** - Explain only the context needed to make the requested
   change clear.
9. **No repetition** - Each observation appears in exactly one section.

### Using Sub-Agents

For medium or large PRs, spawn sub-agents to investigate independent areas:
changed subsystems, expected test locations, public API callers, security
surfaces, or unfamiliar infrastructure. Ask each sub-agent to return concise
evidence and file references, not a full review.

Before finalizing, fact-check each reported issue. For every non-obvious claim,
re-read the relevant code or ask a sub-agent to verify it as **valid**,
**invalid**, or **needs rewording**. Drop invalid issues and reword unclear ones.

## Severity

Give every finding a level. The level measures how hard the harm is to undo,
not how sure you are of the finding.

| Level | Meaning | Merge effect |
|---|---|---|
| **P0** | One-way harm or exposure: lost or wrong funds or balances, deleted data, an upgrade that breaks with no migration path, credentials that left the machine | Blocks. Notify the owner at once. |
| **P1** | Breaks the project's core behavior in a change that reaches users before it can be reverted, or a one-way change with no rollout or recovery plan | Blocks. |
| **P2** | A real defect or gap in a reversible change that tests and real use didn't cover | Doesn't block. Fix it in a follow-up. |
| **P3** | Nice to have: a regression test for behavior already proven in use, naming, docs, refactors | Doesn't block. Batch it later. |

These inputs move a finding's level:

| Input | Effect on the level |
|---|---|
| One-way (can't be reverted) | Raises it |
| Validated by tests and real use | Lowers it |
| Larger blast radius (users, funds, shared state) | Raises it |
| Outside the change's goal | Not a severity: a ticket, never a blocker |

Four rules cover common cases:

- A failing required CI check is not a finding severity. Required checks
  already block the merge.
- A path reachable only through allocation failure or fault injection is P3.
  Link one shared policy issue for the whole class.
- A secret shown in local output is P1. It is P0 once it leaves the machine:
  committed, published, or sent to an outside service.
- Behavior that existed before the PR is a ticket, not a blocker on that PR.

Examples:

- **P0:** a payout uses the wrong terms; a migration drops a column with no
  backup.
- **P1:** a valid order is refused; a shipped command fails on ordinary
  input; a report prints a password to the terminal.
- **P2:** a diagnostic command misses one install layout; an error path
  returns a generic message.
- **P3:** help text and usage line disagree; a test has no lower bound; an
  abort that happens only when an allocation fails.

A project may add its own examples per level. Its repository instructions
override the examples here, not the merge effect.

## Review Workflow

### Step 1: Understand Context

Before reviewing, build a clear picture of what changed and why:

1. Seek the purpose of the change from the PR title and body, commit messages,
   linked issues, and relevant repository docs, even when the request supplies
   only a diff or review target. Follow references that could materially affect
   the assessment. When rationale is missing, investigate before inferring it;
   distinguish documented intent from your reconstruction and surface
   uncertainty when it materially affects the review.
2. Group changes by type: product behavior, API, data model, tests, docs,
   configuration, build, or deployment.
3. Note the size and risk profile: changed files, touched subsystems, public
   interfaces, persisted data, and production paths.
4. Read surrounding code for significantly changed files to understand existing
   patterns and the reasons behind them. Compare the approach with nearby
   implementations and documented conventions.
5. Read prior review discussions for rationale, settled decisions, and open
   questions, as well as concerns already reported. Check their claims against
   the change rather than accepting them as conclusions.

### Step 2: Deep Review

Evaluate each meaningful changed line against the checklist below. Prioritize
bugs and risks over style preferences. Use the wider context to examine the
change's assumptions, fit, and tradeoffs as well as its implementation.

### Step 3: Check Backward Compatibility

Backward compatibility matters for public APIs, persisted data, configuration,
wire protocols, CLI behavior, migrations, user-visible behavior, and stable
integrations.

Check for:
- Removed, renamed, or re-scoped public interfaces.
- Changed function signatures, required parameters, defaults, return shapes, or
  error behavior.
- Changed persisted formats, migration assumptions, or deployment order
  requirements.
- Silent behavior changes that existing users may not notice until production.
- Missing migration path, release note, or compatibility window for intentional
  breaks.

When unsure, ask:
1. Would existing usage break silently?
2. Would existing usage fail loudly but recoverably?
3. Is there a migration path that avoids immediate user action?
4. Is the behavior documented and covered by tests?
5. Does the PR clearly explain why the compatibility risk is acceptable?

### Step 4: Formulate Review

Write findings as concrete requests. Each finding should include the affected
file or behavior, why it matters, and what change would resolve it. Use the
context gathered to substantiate concerns that matter to the change. Omit a
running account of the investigation; broad investigation should still yield
selective findings.

### Step 5: Final Recommendation

Recommend one of:
- **Approve** - No unresolved P0 or P1 finding. Record P2 and P3 findings
  in the review and name them as follow-up tickets.
- **Request Changes** - At least one unresolved finding is P0 or P1 under
  [Severity](#severity).
- **Needs Discussion** - The core issue is design, product intent, migration
  strategy, or ownership and needs agreement before implementation changes.

Rate a missing test like any other finding. A missing test for behavior
already proven in real use is P3. A missing test on a one-way path, or one
that moves funds, is P1.

## Review Checklist

### Correctness

- [ ] The implementation matches the stated product or technical intent.
- [ ] Edge cases, empty inputs, invalid inputs, and failure paths are handled.
- [ ] State transitions are explicit and reversible where needed.
- [ ] Ordering assumptions are documented or enforced by code.
- [ ] Errors are surfaced at the right level with enough context to debug.
- [ ] The change does not introduce stale reads, lost writes, double execution,
      duplicate registration, or cleanup gaps.

### Design And Maintainability

- [ ] Abstractions are clear and necessary for the current requirements.
- [ ] New interfaces have clear ownership, invariants, and lifecycle behavior.
- [ ] Hidden flags, side-channel state, and implicit calling conventions are
      avoided or clearly justified.
- [ ] The code follows established local patterns in the same module or nearby
      subsystem.
- [ ] Helpers are not created for one-off trivial logic unless they improve
      readability meaningfully.
- [ ] Documentation and examples show the correct path directly, not a confusing
      mix of anti-patterns and corrections.

### API And Compatibility

- [ ] Public or stable interfaces remain compatible, or the PR includes a clear
      migration plan.
- [ ] Defaults, required parameters, return values, error types, and side effects
      are unchanged unless intentionally documented.
- [ ] Persisted data, schema changes, and protocol changes are backward and
      forward compatible where deployment ordering requires it.
- [ ] Deprecated paths remain available long enough for users to migrate.
- [ ] Compatibility risks are called out in release notes, docs, or PR text.

### Testing

- [ ] New behavior has focused tests.
- [ ] Bug fixes include regression tests that fail without the fix.
- [ ] Tests are placed near related coverage instead of creating isolated test
      files unnecessarily.
- [ ] Tests cover boundary conditions, negative cases, and representative real
      usage.
- [ ] Assertions verify important outputs and error messages, not just that code
      executes.
- [ ] Tests avoid brittle timing, ordering, environment, and network assumptions.
- [ ] Shared fixtures or helpers reduce duplication without hiding the key test
      behavior.

### Security

- [ ] Inputs crossing trust boundaries are validated, normalized, and bounded.
- [ ] The change does not expose secrets, credentials, tokens, internal URLs, or
      sensitive logs.
- [ ] Authentication, authorization, and tenancy checks remain enforced on every
      path.
- [ ] File paths, shell commands, URLs, redirects, templates, and queries are not
      vulnerable to injection or traversal.
- [ ] Deserialization, dynamic evaluation, plugin loading, and dependency changes
      do not expand the attack surface unnecessarily.
- [ ] CI, deployment, and release changes do not grant untrusted code access to
      sensitive credentials or privileged environments.

### Concurrency And Reliability

- [ ] Shared mutable state is synchronized or confined to one owner.
- [ ] Multiple locks or resources are acquired in a consistent order.
- [ ] Retries are idempotent or have safeguards against duplicate side effects.
- [ ] Timeouts, cancellation, cleanup, and partial failure behavior are defined.
- [ ] Background jobs, queues, listeners, and subscriptions cannot leak or run
      after their owner is gone.
- [ ] Distributed or multi-process behavior handles race conditions and stale
      state.

### Performance

- [ ] Hot paths avoid unnecessary allocations, repeated parsing, excessive I/O,
      and avoidable network calls.
- [ ] Loops and queries scale with expected data size.
- [ ] Caches have clear invalidation, bounds, and ownership.
- [ ] Large payloads are streamed, paginated, batched, or bounded where needed.
- [ ] The PR includes benchmarks or measurements for risky performance-sensitive
      changes.

### Observability And Operations

- [ ] Important failures have logs, metrics, traces, or events at the right
      level.
- [ ] Logs include useful context without leaking sensitive data.
- [ ] Feature flags, migrations, rollouts, and rollback paths are clear for
      production changes.
- [ ] Configuration changes have safe defaults and validation.
- [ ] Alerts or dashboards are updated when production behavior changes.

## Output Format

Structure reviews like this. Omit sections where there are no problems to
report. Do not write "No concerns" or affirmative filler.

```markdown
## PR Review: #<number>
<!-- Or for local branch reviews: -->
## Branch Review: <branch-name> (vs main)

### Summary
What the PR does in one sentence, followed by the overall verdict.

### Correctness
[Problems only]

### Design And Maintainability
[Problems only]

### API And Compatibility
[Problems only]

### Testing
[Problems only]

### Security
[Problems only]

### Concurrency And Reliability
[Problems only]

### Performance
[Problems only]

### Observability And Operations
[Problems only]

### Recommendation
**Approve** / **Request Changes** / **Needs Discussion**

[Brief justification focused on what blocks approval, if anything]
```

### Specific Comments

Only include this section if the user requests a detailed or in-depth review.
Do not repeat observations already made in other sections. Use it for additional
file-specific feedback that does not fit the categorized sections.

```markdown
### Specific Comments
- `src/module.ts:42` - This branch does not handle an empty result set, so the caller receives a success response with incomplete data.
- `test/module.test.ts:100-105` - Add a regression test for the error path introduced by this change.
```

## Files To Reference

When reviewing, consult project files for context rather than relying on memory.
Common examples include:

- Repository instructions such as `README.md`, `CONTRIBUTING.md`, `CLAUDE.md`,
  `AGENTS.md`, or `.cursor/rules/`.
- Existing tests near the changed code.
- Nearby implementations of the same pattern.
- API docs, migration docs, release notes, or schema definitions touched by the
  PR.
- Build, CI, deployment, and configuration files changed by the PR.
