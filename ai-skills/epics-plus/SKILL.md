---
name: epics-plus
description: Discover, read, and manage Epics+ markdown documents in the boku repository. Use when asked about active tasks, priorities, epics for specific repos or tags, or when needing to regenerate the epic index (INDEX.md) using the epics-plus CLI.
---

# Epics+ Management

Navigate, interpret, and manage Epics+ docs in the boku repository. Use this skill to understand the status and priority of work, and to use the `epics-plus` tooling.

## Quick Start and Usage Modes

1. **Check active work**: Read [epics/INDEX.md](/home/realu/ki/boku/epics/INDEX.md) for the current dashboard of active epics grouped by priority.
2. **Verify/Scan**: To see all epics with frontmatter or validate them:
   ```bash
   cd /home/realu/ki/boku/epics && just epics-plus scan --fm-only --strict
   ```
3. **Regenerate index**: If you've modified frontmatter or added an epic, update the dashboard:
   ```bash
   cd /home/realu/ki/boku/epics && just epics-plus index
   ```

## Interpreting Epics+ Frontmatter (Reader View)

When reading an epic doc, look at the YAML frontmatter at line 1 for context:

- **status**: `active` | `inactive` | `done` | `archived`.
- **priority**: `p0` (urgent) to `p3` (someday). `p2` is default.
- **epic_kind**: `spec` | `journal` | `reference`.
- **tags**: Broad themes (e.g., `sai`, `evm`, `slashing`). Use for "related work" queries.
- **repos**: Which codebase this epic targets (e.g., `nibi-chain`, `sai-keeper`).
  These use the conventional repo names from the `/repo-map` skill.
- **related_context**: Links to other epics (`/epics/...`). Follow these to understand dependencies.
- **agent_skills**: Cursor skill IDs relevant to this epic. Suggest using them when working on the task.

## Commands Reference

Run from `/home/realu/ki/boku/epics`:

- `just epics-plus index`: Writes `INDEX.md`.
  - `--dry-run`: Print to stdout instead.
  - `--all-statuses`: Include inactive/done/archived epics.
- `just epics-plus scan`: List epics.
  - `--fm-only`: Skip files without Epics+ YAML.
  - `--strict`: Exit non-zero if validation fails.
  - `--format ndjson`: Useful for batch processing.

- Index generation dashboard: [epics/INDEX.md](/home/realu/ki/boku/epics/INDEX.md)

## Additional Resources
- Full schema and semantics: [epics/26-02-24-epics-plus.md](/home/realu/ki/boku/epics/26-02-24-epics-plus.md)

## Mode: Finding or Searching for Epics

Use this procedure when the user asks to find, search, locate, resolve, or get
the path for an epic.

Example invocations:
```
/epics-plus Find me the recent epic on publishing my Go release on npm
/epics-plus Search for the epic on creating new markets on Sai
```

### Procedure

First, query the lightweight Epics+ metadata:

```bash
cd /home/realu/ki/boku
just epics-plus scan --fm-only --json path,title,data
```

Use the returned JSON to search across:
- `path`
- `title`
- `data.title`
- `data.status`
- `data.priority`
- `data.epic_kind`
- `data.created`
- `data.tags`
- `data.repos`
- `data.related_context`
- `data.agent_skills`

Prefer active epics by default, unless the user explicitly asks for archived,
done, inactive, or all epics.

If the metadata scan is not enough, also search within:

```bash
/home/realu/ki/boku/epics
```

Search file paths, headings, and body text as needed.

### Ranking

Prefer matches in this order:
- Strong title or filename/path match.
- Query terms appearing in `data.tags`, `data.repos`, or `data.agent_skills`.
- Query terms appearing in `data.related_context`.
- Recent `data.created` date when the user asks for recent work.
- Higher priority when otherwise tied: `p0`, `p1`, `p2`, `p3`.

### Output

If one clear epic matches, report the absolute path in a code block:

```txt
/home/realu/ki/boku/epics/26-05-05-go-cli-npm.md
```

If multiple epics may match, report all plausible absolute paths in a code block,
then add a short note with titles so the user can choose:

```txt
/home/realu/ki/boku/epics/26-03-25-sai-new-markets/README.md
/home/realu/ki/boku/epics/26-03-25-sai-new-markets/mktg-go-live.md
```

When no match is found, say that no matching epic was found and mention whether
you searched metadata only or also searched the epics directory body text.

### Applying Search Filters by Status 

By default, `scan` returns active epics only. When the user asks for other
statuses or a different result window, use long-form flags:

```bash
# Find up to 20 active epics (default status and limit)
just epics-plus scan --fm-only --json path,title,data

# Find up to 50 active epics
just epics-plus scan --fm-only --limit 50 --json path,title,data

# Find done epics
just epics-plus scan --fm-only --status done --limit 20 --json path,title,data

# Find inactive epics
just epics-plus scan --fm-only --status inactive --limit 20 --json path,title,data

# Find inactive or done epics
just epics-plus scan --fm-only --status inactive,done --limit 50 --json path,title,data

# Remove status filtering and search all frontmatter epics
just epics-plus scan --fm-only --status all --limit 100 --json path,title,data
```

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

