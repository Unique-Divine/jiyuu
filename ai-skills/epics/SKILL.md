---
name: epics
description: Discover, read, create, edit, normalize, and synthesize Epics+ markdown documents in the boku repository. Use for active tasks, priorities, epics for specific repos or tags, durable epic/spec handoffs, or regenerating INDEX.md with the epics-plus CLI.
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
- **related_context**: Links to other epics (`/epics/...`). Follow these for
  relevant parent, child, and peer context.
- **agent_skills**: Cursor skill IDs relevant to this epic. Suggest using them when working on the task.

## Epic Structure and Creation

Epics+ metadata stays flat. Frontmatter identifies a Markdown document as an
epic; its path indicates whether it is standalone, a parent, or a child:

- The unqualified term **epic** may refer to any of these three forms. An
  **epic document** is any Markdown file with valid Epics+ frontmatter,
  including a parent `README.md` or a child epic file. Do not assume that every
  request for an epic means one standalone file; infer a file or directory from
  the number of cohesive outcomes and related files involved.
- A **standalone epic** is one frontmatter-bearing Markdown file directly under
  `epics/`.
- A **parent epic** is the frontmatter-bearing `README.md` at the root of an
  epic directory. It is the canonical entry point and indexes the child epics.
- A **child epic** is a frontmatter-bearing Markdown file within an epic
  directory. It represents an independently trackable outcome within the
  parent epic.
- Other files may live beside the epic documents. Without Epics+ frontmatter,
  treat them as context, artifacts, or implementation material rather than
  additional epics.

Do not add hierarchy fields such as `epic_role`, `parent_epic`, or
`depends_on`. Use field `related_context` for lightweight relationships and
follow the file layout for parent-child context.

### Choosing a file or directory

- Use a standalone file when one document can describe one cohesive outcome.
- Use a directory when the work benefits from multiple independently trackable
  child epics or needs related files.
- For a directory, create `README.md` as the parent epic and list each child
  epic with a short statement of the outcome it owns.
- Name child epics with stable numeric prefixes such as `01-...md`,
  `02-...md`, and `03-...md`. The numbers define reading and display order;
  they do not require completion in that sequence.

### Epics, tasks, and subtasks

An epic document describes an outcome. Track actionable work inside it with
standard Markdown checkboxes:

- Use `- [ ]` for open tasks and `- [x]` for completed tasks.
- Indent checkbox items beneath a task to represent subtasks.
- Use ordinary bullets only for non-actionable context, rationale, status,
  examples, or notes.
- If an item describes work to perform or an acceptance criterion to verify,
  write it as a checkbox rather than hiding it in an ordinary bullet.

When creating or restructuring task lists, follow agent skill `md-tasks`.

### Spec synthesis and handoff

For a design-heavy epic, use agent skill `drill-spec` to resolve material
decisions and write them back as the discussion progresses. Once the design
stabilizes for an implementation sequence, synthesize the epic before calling
that scope ready for implementation:

- Preserve useful context, evidence, rationale, caveats, and settled decisions.
  Do not reduce the drill history to a bare checklist.
- Follow agent skill `md-tasks` for a final normalization pass. Keep settled
  decisions distinct from the open implementation, validation, rollout, or
  deferred tasks that follow from them.
- Place executable and verifiable work in one or more top-level `## Impl`,
  `## Impl N: ...`, validation, rollout, or deferred sections. Multiple
  top-level implementation sections are useful when the work has distinct
  sequences or outcomes; preserve an epic's useful existing structure.
- Explicitly defer non-blocking questions. A stable implementation sequence may
  proceed without implying that every later sequence is fully specified.
- Add concise logical or ASCII flows when they materially clarify execution,
  but do not require them mechanically.

Before presenting an epic or implementation sequence as ready, check that:

- No material open question is presented as settled.
- Settled decisions retain their rationale and are not mistaken for completed
  implementation.
- Each actionable consequence is an open implementation, validation, rollout,
  or explicitly deferred task.
- No actionable work remains hidden in ordinary prose or bullets.

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
- Codex runtime skill discovery: [references/codex-skill-discovery.md](./references/codex-skill-discovery.md)

## Mode: Finding or Searching for Epics

Use this procedure when the user asks to find, search, locate, resolve, or get
the path for an epic.

Example invocations:
```
/epics Find me the recent epic on publishing my Go release on npm
/epics Search for the epic on creating new markets on Sai
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

## Operator note: managed agent skills

Public skills live in `jiyuu/ai-skills`; private skills live in
`boku/priv-skills`. Cursor and Codex both resolve their runtime directories to
the combined `priv-skills` directory, so runtime edits update the repositories
immediately. Run `just health` from `boku/dotfiles` to check the public/private
union and both runtime links without writing changes.
