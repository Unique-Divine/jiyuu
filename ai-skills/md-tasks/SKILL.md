---
name: md-tasks
description: >-
  Apply consistent Markdown checkbox task conventions in READMEs, epics/journals,
  specs, and design docs. Use when the user wants task lists with "- [ ]" /
  "- [x]", nested subtasks, context sub-bullets under items, or terminal
  commands (grep/rg) to list open versus completed tasks in Markdown files —
  especially in repo documentation and planning Markdown.
---

# Markdown tasks and checkbox conventions

Follow these conventions so agents and humans can scan open work reliably.

## Usage guidelines

### Checkbox notation

Denote open tasks or TODOs with standard checkbox notation:

- `- [ ]` for incomplete tasks.
- `- [x]` for done tasks.

Indentation defines subtasks: extra leading spaces nest bullets under a parent item.

In planning/spec documents, default every actionable item at every nesting level
to checkbox notation. Do not hide implementation work in ordinary bullets.

Often add ordinary sub-bullets for status or context beneath a task, but only
when the bullet is not itself work to do. For example:

```markdown
- [ ] Some task
  - Status: maybe some status description or how it was completed
  - More context
  - [ ] Subtask A
```

This keeps review tight: unfinished work sorts visually to checklist lines while
still allowing narrative detail.

### Actionable sub-bullets

If a sub-bullet describes work to do, a decision to resolve, behavior to
implement, a file to create, a test to add, or an acceptance criterion to verify,
write it as a checkbox task too:

```markdown
- [ ] Implement tx archive CLI.
  - [ ] Keep Bash responsible for chain config, keyring preflight, and broadcast.
  - [ ] Make TypeScript responsible for txhash parsing and Markdown formatting.
  - Rationale: TypeScript is easier to test for JSON, filenames, and CLI args.
```

Use ordinary nested bullets only for non-actionable context, rationale, status,
examples, or notes. When unsure whether a sub-bullet is actionable, prefer
`- [ ]`.

### Task Questions

Sometimes we'll pose a question or frame a resolve decision as a task. That can
look like this.

```markdown
- [ ] Q: Some open question?
  A: An answer on what to do. The question task is complete when each of the
  potential subtasks is completed or the answer, "A: ..." is implemented.
```

### Finding tasks in the terminal

Patterns differ by shell (`**/*.md` is not universal glob). Typical approaches:

```bash
# Completed tasks under a directory (GNU grep fallback)
grep -rF -- '- [x]' <path-or-glob>

# Open tasks — limit to Markdown (example: cwd tree)
grep -rF -- '- [ ]' . --include='*.md'
```

Or use **`rg`** for `**` globs and filters:

```bash
rg --fixed-strings -g '*.md' -- '- [ ]'
rg --fixed-strings -g '*.md' -- '- [x]'
```

Tune paths and `-g`/globbing to match the repo layout.
