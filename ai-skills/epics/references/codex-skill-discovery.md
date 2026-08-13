# Coding-agent runtime skill discovery

Use this reference when an agent needs to verify that Codex or Cursor Agent
discovered a repository-local or user-managed skill.

## Inspect the model-visible skill catalog

Run command `codex debug prompt-input` from the repository root. It renders the
prompt inputs that Codex would receive, including the `Available skills` list.
Filter its JSON output instead of printing the full prompt, which can also
contain project instructions and environment context.

```bash
codex debug prompt-input | rg -n -C 2 'epics'
```

For a repository-local skill, replace `epics` with its exact skill name. The
matching entry should show the skill description and the resolved `SKILL.md`
path. For example, a skill under directory `.agents/skills/sai-web/` should
appear with its repository-local source path when Codex starts from that
checkout.

Command `codex --help` documents available CLI commands but does not list
discovered skills. Command `codex doctor --json` diagnoses installation and
configuration health, but it also does not render the model-visible skill
catalog.

## Probe Cursor Agent skill discovery

Cursor Agent does not expose a local `debug prompt-input` or skill-list command
in its CLI help. Use a short, non-interactive, read-only agent request as the
discovery probe instead. Run it from the target repository so its workspace
skill search uses the same context as the coding task.

```bash
agent --print --mode ask --output-format text --trust \
  "List the skills available to you in this workspace. For each, give only its exact name and source path. Do not inspect files, run commands, or make changes."
```

The result should list the skill name and resolved `SKILL.md` path. A
repository-local skill under `.agents/skills/sai-web/`, for example, should
appear with that repository-local path. Cursor Agent may also list its built-in
skills and managed skills under `$HOME/.agents/skills`; that is expected.

This command makes a short Cursor Agent request. It is a discovery check, not a
fully local prompt renderer like command `codex debug prompt-input`.

## Managed skill synchronization

Public skills live in `jiyuu/ai-skills` and private skills in
`boku/priv-skills`. The runtime Cursor and Codex skill directories both link to
the flat `priv-skills` union, so edits apply to the canonical repository files
immediately. Run `just skills-sync --run` only to repair the managed links.

After syncing, run the relevant Codex or Cursor Agent discovery probe from a
representative target repository. That final check verifies discovery from the
same working-directory context in which the next coding agent will operate.
