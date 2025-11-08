# jiyuu/discord

Discord moderation helpers for the Nibiru community, written in TypeScript on top
of `discord.js`.

## Setup

1. Copy the environment template and add your bot token.
   ```bash
   cp .env.example .env
   ```
2. Install dependencies (Bun is the default runtime/tooling).
   ```bash
   bun install
   ```
3. Start the bot.
   ```bash
   bun run src/main.ts
   # or
   just dev
   ```

Runtime artifacts (channel/member dumps, etc.) are written to `.tmp/`, which is
ignored by Git.

## Commands & Status

- `all-roles`: list guild roles (writes to `.tmp/saveJson.json`).
- `all-channels`: list guild channels.
- `grant-roles`: experimental flow to grant validator roles based on channel
  history.
- `members`: paginate guild members for inspection.
- `echo`: simple echo helper for testing.

See `plan.md` for the latest clean-up roadmap and remaining TODOs.
