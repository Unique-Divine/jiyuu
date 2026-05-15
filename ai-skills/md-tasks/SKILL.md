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

Often add ordinary sub-bullets for status or context beneath a task, for example:

```markdown
- [ ] Some task
  - Status: maybe some status description or how it was completed
  - More context
  - [ ] Subtask A
```

This keeps review tight: unfinished work sorts visually to checklist lines while
still allowing narrative detail.

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
