---
name: new-skills
description: Create or revise agent skills, organize SKILL.md content with progressive disclosure, choose the correct public or private canonical location, and validate skill frontmatter. Use when the user wants to create, restructure, simplify, or improve a skill or its trigger description. Do not start an evaluation or test-prompt loop unless the user explicitly requests one.
---

# New skills

Create and revise agent skills as concise operational references. Start from the
user's actual workflow, preserve useful existing material, and validate the
result without introducing an evaluation process the user did not request.

## Understand the intended skill

Use the conversation and existing files to establish:

1. What work the skill should enable.
2. Which requests should and should not trigger it.
3. What tools, inputs, outputs, and repository conventions it depends on.
4. Whether it is public or private.

Inspect relevant skills and source documentation before asking questions that
the environment can answer. Ask the user only about unresolved intent or
tradeoffs that materially change the skill.

When revising an existing skill, read its complete `SKILL.md` and each bundled
resource needed for the requested change. Preserve its directory name and
frontmatter field `name` unless the user explicitly requests a rename.

## Choose the canonical location

This workstation keeps public and private skills in different repositories:

- Public: `/home/realu/ki/boku/jiyuu/ai-skills/<name>`
- Private: `/home/realu/ki/boku/priv-skills/<name>`

Directories `~/.agents/skills` and `~/.cursor/skills` are managed symlinked views
of the combined skill set. Editing through either view changes the canonical
repository file immediately, but resolve the path before reporting which
repository owns the skill.

Use a private skill for personal data, private operations, credentials-adjacent
workflows, or instructions that should not be published. Mark it explicitly:

```yaml
metadata:
  private: true
```

Public skills omit `metadata.private` rather than setting it to `false`.

Command `just skills-sync --run` from repository `boku/dotfiles` repairs the
managed union and runtime links. It is not a required post-edit copy step when
the links are already healthy.

## Write the skill

Every skill directory requires `SKILL.md` with YAML frontmatter:

```yaml
---
name: example-skill
description: State what the skill does and the concrete requests or contexts in which it should be used.
---
```

The description is the trigger contract. Include both capability and trigger
conditions there. Keep nearby tasks that should use another skill out of its
claimed scope.

Write the body in imperative language and explain enough rationale for an agent
to apply the guidance outside the initial example. Prefer concrete workflows and
examples over a glossary of commands.

### Apply progressive disclosure

Keep the root file focused on routing and behavior common to every invocation:

```text
skill-name/
├── SKILL.md
├── references/   # Detailed guidance loaded only for a matching workflow
├── scripts/      # Deterministic helpers used repeatedly
└── assets/       # Templates or output resources
```

- Put trigger information in frontmatter field `description`.
- Keep `SKILL.md` concise; under 500 lines is a useful ceiling, not a target.
- Move variant-specific or detailed material into `references/` and tell the
  agent exactly when to read each file.
- Give a reference longer than 300 lines a short table of contents.
- Add a script only when deterministic or repeated work justifies maintaining
  it. Prefer an existing repository command surface over duplicating it.
- Reuse existing assets and templates instead of recreating them.

Avoid repeating the same workflow in the introduction, implementation section,
and closing summary. Remove conversational filler, claims about model
intelligence, and instructions tied to an unavailable agent interface.

## Validate frontmatter

Run the bundled validator through `uv`; do not invoke it with bare `python` or
`python3`:

```bash
uv run --with pyyaml python \
  "$HOME/.agents/skills/new-skills/scripts/quick_validate.py" \
  /absolute/path/to/skill
```

If command `uv` is unavailable, report the missing dependency instead of
installing software without permission.

The validator checks `SKILL.md` frontmatter and the private-skill marker. It does
not judge whether the instructions are correct or useful. Inspect links and
referenced files separately when the edit changes progressive-disclosure
routing.

Do not create test prompts, workspaces, transcripts, isolated agent runs, or
evaluation artifacts unless the user explicitly asks for evaluation work.

## Finish the revision

Re-read the skill once for trigger accuracy, progressive disclosure, stale
paths, and unnecessary repetition. Report the canonical files changed, the
validation performed, and any intentionally deferred questions.
