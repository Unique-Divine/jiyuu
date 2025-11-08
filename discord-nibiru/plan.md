# plan.md - discord-nibiru

## Definitions
- `x -` denotes a completed item. Use within headings or list entries.
- `o -` denotes an incomplete item. Use within headings or list entries.
- Indent sub-items with two spaces and maintain the prefix format (e.g. `  - x - task`).
- Headings keep their Markdown level; the prefix follows the hashes (e.g. `## x - Section`).

## 2025-11-08 Future Potential Work
  1. o - Refactor command registration into a dedicated module; keep `main.ts` focused on events and share common context across commands.
  2. o - Add scenario-based tests using Bun’s runner with real payload fixtures to assert replies and JSON outputs.
  3. o - Centralize channel/role identifiers in typed config modules to avoid magic strings and catch drift at compile time.
  4. o - Add CLI helpers for safe manual inspection (e.g. `bun run scripts/inspect.ts`) so team members can fetch channel/role data without running the full bot.
  5. o - Make destructive commands (bans/deletes) support `--dry-run` and emit `.tmp/` summaries before executing to create a consistent audit trail.
  6. o - Wrap `logModerationAction` with structured logging/metrics output to JSON so moderation events are easy to review or ingest later.