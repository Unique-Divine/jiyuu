---
name: epics-plus
description: Discover, read, and manage Epics+ markdown documents in the boku repository. Use when asked about active tasks, priorities, epics for specific repos or tags, or when needing to regenerate the epic index (INDEX.md) using the epics-plus CLI.
---

# Epics+ Management

Navigate, interpret, and manage Epics+ docs in the boku repository. Use this skill to understand the status and priority of work, and to use the `epics-plus` tooling.

## Quick Start

1. **Check active work**: Read [epics/INDEX.md](/home/realu/ki/boku/epics/INDEX.md) for the current dashboard of active epics grouped by priority.
2. **Regenerate index**: If you've modified frontmatter or added an epic, update the dashboard:
   ```bash
   cd /home/realu/ki/boku/epics && just epics-plus index
   ```
3. **Verify/Scan**: To see all epics with frontmatter or validate them:
   ```bash
   cd /home/realu/ki/boku/epics && just epics-plus scan --only-with-frontmatter --strict
   ```

## Interpreting Epics+ Frontmatter (Reader View)

When reading an epic doc, look at the YAML frontmatter at line 1 for context:

- **status**: `active` | `inactive` | `done` | `archived`.
- **priority**: `p0` (urgent) to `p3` (someday). `p2` is default.
- **epic_kind**: `spec` | `journal` | `reference`.
- **tags**: Broad themes (e.g., `sai`, `evm`, `slashing`). Use for "related work" queries.
- **repos**: Which codebase this epic targets (e.g., `nibi-chain`, `sai-keeper`).
- **related_context**: Links to other epics (`/epics/...`). Follow these to understand dependencies.
- **agent_skills**: Cursor skill IDs relevant to this epic. Suggest using them when working on the task.

## Recommended Workflow

### 1. Triage / "What's Active?"
Read [epics/INDEX.md](/home/realu/ki/boku/epics/INDEX.md). If you think it might be stale, run `just epics-plus index --dry-run` to see the current state without writing.

### 2. Deep Dive / Context Gathering
When assigned to an epic, open the file and:
- Check `related_context` and `tags` to find relevant background docs.
- Use `repos` to identify the target codebase and cross-reference with the `repo-map` skill.
- Check `agent_skills` to see if specific domain expertise is required.

### 3. Maintenance
Regenerate the index after any frontmatter changes so the dashboard stays accurate.

## Commands Reference

Run from `/home/realu/ki/boku/epics`:

- `just epics-plus index`: Writes `INDEX.md`.
  - `--dry-run`: Print to stdout instead.
  - `--all-statuses`: Include inactive/done/archived epics.
- `just epics-plus scan`: List epics.
  - `--only-with-frontmatter`: Skip files without Epics+ YAML.
  - `--strict`: Exit non-zero if validation fails.
  - `--format ndjson`: Useful for batch processing.

## Additional Resources
- Full schema and semantics: [epics/26-02-24-epics-plus.md](/home/realu/ki/boku/epics/26-02-24-epics-plus.md)
- Index generation dashboard: [epics/INDEX.md](/home/realu/ki/boku/epics/INDEX.md)
