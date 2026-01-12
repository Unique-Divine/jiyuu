# Epics+ Reference

Detailed reference for the Epics+ system in boku.

## Schema & Semantics
The canonical source for the Epics+ v1 schema is:
[epics/26-02-24-epics-plus.md](/home/realu/ki/boku/epics/26-02-24-epics-plus.md)

### Key Field Meanings (Reader View)
- **status**:
  - `active`: In motion; expect to touch this again.
  - `inactive`: Paused, but still relevant.
  - `done`: Complete; useful as reference.
  - `archived`: Intentionally out of circulation.
- **priority**:
  - `p0`: Urgent / drop-everything.
  - `p1`: Important, should move soon.
  - `p2`: Normal / default.
  - `p3`: Nice-to-have / someday.

## Index Generation (INDEX.md)
The file [epics/INDEX.md](/home/realu/ki/boku/epics/INDEX.md) is a generated dashboard.

- **Grouping**: Active epics are grouped by `priority` (p0 → p3).
- **Links**: Links in the index use the `/epics/` prefix as per the project's standard.
- **Regeneration**: If the file feels stale, run `cd /home/realu/ki/boku/epics && just epics-plus index`.

## Relationship to Other Agents/Skills
- **epics-frontmatter agent**: Used for *writing* frontmatter (adding/updating YAML).
- **repo-map skill**: Use the `repos` field in an epic's frontmatter to find the target repository in the `repo-map` skill.
- **agent_skills**: These fields in the frontmatter should be treated as suggestions for which skills to apply when working on the epic's tasks.
