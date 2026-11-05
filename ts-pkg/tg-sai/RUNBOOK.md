# Sai Telegram admin bootstrap runbook

Use this when Claude web + MCP is not enough. MCP admin tools can mis-report
roles on **Group (Basic)** chats. This folder holds a TypeScript + mtcute tool
that uses the correct Telegram API paths and keeps decision logic testable
offline.

Location: `jiyuu/ts-pkg/tg-sai/`

## What the tool does today

Command `just tg-sai` exposes a Commander CLI for one selected Telegram chat at
a time. The command prints help and exits with status code `0` when called with
no arguments.

Implemented read-only behavior:

- Command `tg-sai audit` verifies the active session user with `getMe()`.
- Command `tg-sai audit` checks the session user's role in the target chat.
- Command `tg-sai audit` checks a target user or bot role in the target chat.
- Function `planAdminBootstrap` returns proposed add, promote, skip, and noop
  actions without Telegram writes.

Implemented write behavior:

1. Command `tg-sai add-bot --run` adds bot `SaiTeam_bot` if missing.
2. Command `tg-sai add-bot --run` promotes bot `SaiTeam_bot` to admin when
   needed.
3. Command `tg-sai add-owner --run` adds a target user if missing.
4. Command `tg-sai add-owner --run` promotes a target user to admin when needed.
5. Command `tg-sai add-owner --run` transfers ownership only when environment
   variable `TELEGRAM_PASSWORD` exists and the target user is eligible.

Credential profile behavior:

- Command `tg-sai profile find` reports discovered Telegram credential sources
  without writing profile state.
- Bare commands `tg-sai profile` and `tg-sai profile add` show help and configured
  profiles without saving state or connecting to Telegram.
- Command `tg-sai profile add` with explicit options stores a local profile from
  flags, process environment variables, or merged non-conflicting discovered
  sources. Use a distinct `--name` for another account; without it, explicit
  options update the active/default profile.
- Explicit `--qr`, `--phone`, or `--login` starts a fresh account login and
  ignores discovered/environment sessions and account passwords. App credentials
  can still come from discovery, environment, or interactive input.
- Commands `tg-sai profile list`, `tg-sai profile show`, `tg-sai profile use`,
  and `tg-sai profile remove` manage local session profiles.

Default target identities live in file `cfg.ts`: constant `saiBot` and
constant `uniqueDivine`.

## Credentials

The CLI resolves Telegram credentials in this order:

1. Command flags where supported.
2. Environment variables `TELEGRAM_API_ID`, `TELEGRAM_API_HASH`, and
   `TELEGRAM_SESSION_STRING`.
3. Active profile in file `~/.tg-sai/profiles.json`.
4. Repo-local file `jiyuu/ts-pkg/tg-sai/.env` and user-local file
   `~/.tg-sai/.env`.
5. OS-aware discovery from known MCP config files.

The tool stores profiles, logs, and generated operator files under directory
`~/.tg-sai/` by default. Tests and one-off runs can override the tool directory
with environment variable `TG_SAI_DIR`.

Discovery checks the two `tg-sai` `.env` files first, then these user-level
config paths by platform:

- macOS: Claude Desktop file
  `~/Library/Application Support/Claude/claude_desktop_config.json`, Cursor
  file `~/.cursor/mcp.json`, and Claude Code file `~/.claude.json`.
- Linux: Cursor file `~/.cursor/mcp.json` and Claude Code file
  `~/.claude.json`.
- Windows: Cursor file `~/AppData/Roaming/Cursor/User/mcp.json` and Claude Code
  file `~/.claude.json`.

Discovery first parses config files as JSON and searches for objects containing
Telegram credential keys. If JSON parsing fails or finds no complete object, it
falls back to line-by-line scanning for JSON-like and shell-like assignments.

Optional elevated mode value: `TELEGRAM_PASSWORD`. Profiles can store the
Telegram 2FA password in file `~/.tg-sai/profiles.json`, and environment
variable `TELEGRAM_PASSWORD` overrides the stored profile value for one command.
When a password is available, command `tg-sai ops sai-bootstrap --run` attempts
ownership transfer by default for eligible chats after the target user is admin.
Pass flag `--no-transfer-owner` to skip ownership transfer.

## Generate a Telegram session string

Use this flow when the operator does not already have a
`TELEGRAM_SESSION_STRING` in an MCP config file. File
`session_string_generator.py` generates the session string with QR login, then
the value is saved into a local `tg-sai` profile.

Install the local command runtimes:

```bash
curl -fsSL https://bun.sh/install | bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Install the Python helper dependencies:

```bash
cd /path/to/tg-sai
uv sync
```

Create user-local file `~/.tg-sai/.env` with Telegram app credentials:

```bash
mkdir -p ~/.tg-sai
chmod 700 ~/.tg-sai
cat > ~/.tg-sai/.env <<'EOF'
TELEGRAM_API_ID=<api id from https://my.telegram.org/apps>
TELEGRAM_API_HASH=<api hash from https://my.telegram.org/apps>
EOF
```

To get those values, open `https://my.telegram.org/apps`, log in with the
Telegram phone number, create an app if needed, then copy field `api_id` into
environment variable `TELEGRAM_API_ID` and field `api_hash` into environment
variable `TELEGRAM_API_HASH`. These app credentials are not the session string.
The session string is generated after QR login.

Generate the session string and save it directly into a `tg-sai` profile:

```bash
tg-sai profile add --name sai-custody --login --qr --interactive
```

Scan the QR code in Telegram with **Settings > Devices > Link Desktop Device**.
Scan while signed into the intended account. After login succeeds, this command
saves its verified identity and session in the separate `sai-custody` profile.
The existing active profile stays selected. Inspect the result with command
`tg-sai profile show sai-custody`.

If you generated the session string manually, save it with explicit flags:

```bash
tg-sai profile add \
  --name sai-custody \
  --api-id <api id> \
  --api-hash <api hash> \
  --session-string '<session string>'
```

Store the Telegram 2FA password in the same profile when ownership transfer is
needed:

```bash
tg-sai profile add --name sai-custody --password '<password>'
```

Check the local setup:

```bash
tg-sai health
```

## Commands

Show help:

```bash
tg-sai
```

Find Telegram credential sources:

```bash
tg-sai profile find
```

Add a local profile from a numbered source shown by command
`tg-sai profile find`:

```bash
tg-sai profile add --name another-account --src 1
```

Command `tg-sai profile add` can store the Telegram 2FA password with flag
`--password`. Command `tg-sai profile show` redacts stored `apiHash`,
`sessionString`, and `password`.

For password or metadata updates, `--name` selects an existing saved profile by
name, handle, numeric user ID, or saved-list index. The update preserves that
profile's account identity and session without importing environment or discovered
credentials. An unknown password-update selector fails instead of creating a
profile. Explicit login/import cannot replace a known profile with a different
account; use a new name for another account, or remove the old profile first.
Flag `--src` selects a credential source numbered by `tg-sai profile find`, not
the saved-profile index printed by `tg-sai profile list`.

Create a blank draft profile for onboarding:

```bash
tg-sai profile add --name draft
```

Blank or partial profiles are useful while collecting `apiId`, `apiHash`, and
`sessionString`. Commands that connect to Telegram require all three fields to
be filled.

Add a local profile from explicit values:

```bash
tg-sai profile add \
  --name another-account \
  --handle <telegram-handle> \
  --user-id <telegram-user-id> \
  --display-name "<display name>"
```

List local profiles:

```bash
tg-sai profile list
```

Command `tg-sai profile` also shows configured profiles beneath its command
list. Bare command `tg-sai profile add` explains fresh QR enrollment and how
named accounts differ from updates to the active profile. Neither changes the
saved configuration.

Find Sai-related chats visible to the active Telegram session:

```bash
tg-sai ops sai-find
```

Command `tg-sai ops sai-find` always writes a JSONL file under directory
`~/.tg-sai/logs/`, using the next available filename such as
`sai-find-00.jsonl` or `sai-find-01.jsonl`. Use flag `--out <path>` to also
write a copy somewhere else.

Dry-run Sai bot and owner-target updates from a reviewed chat file:

```bash
tg-sai ops sai-bootstrap --file ~/.tg-sai/logs/sai-find-00.jsonl
```

Command `tg-sai ops sai-bootstrap` always writes a JSONL run log under directory
`~/.tg-sai/logs/`, using the next available filename such as
`sai-bootstrap-00.jsonl` or `sai-bootstrap-01.jsonl`. The command waits one
second between chats by default. Use flag `--delay-ms <ms>` to change that
delay. When environment variable `TELEGRAM_PASSWORD` is available, execution
mode attempts ownership transfer by default for eligible chats.

Run a live read-only audit:

```bash
tg-sai audit --chat <chat-id>
```

Offline unit tests (no Telegram network):

```bash
bun test adminAudit.test.ts profileCli.test.ts profiles.test.ts
```

Add-bot dry-run for one selected chat:

```bash
tg-sai add-bot --chat <chat-id>
```

Execute bot add/promote for one selected chat:

```bash
tg-sai add-bot --chat <chat-id> --run
```

Add-owner dry-run for one selected chat:

```bash
tg-sai add-owner \
  --chat <chat-id> \
  --target-user <target-user>
```

Execute add-owner add/promote for one selected chat:

```bash
tg-sai add-owner \
  --chat <chat-id> \
  --target-user <target-user> \
  --run
```

Execute add-owner with ownership transfer enabled:

```bash
TELEGRAM_PASSWORD="<2fa password>" tg-sai add-owner \
  --chat <chat-id> \
  --target-user <target-user> \
  --run
```

## File roles

| File | Role |
| --- | --- |
| `cfg.ts` | Production bot/user IDs and handles |
| `main.ts` | Commander CLI for audit, add-bot, and add-owner commands |
| `adminCheck.ts` | Legacy direct audit runner |
| `mtcuteAdapter.ts` | Live mtcute boundary; normalizes chat/member data |
| `adminAudit.ts` | Pure types and decision logic (testable) |
| `profiles.ts` | Profile storage and OS-aware credential discovery |
| `testdata.ts` | Captured fixtures from live runs |
| `adminAudit.test.ts` | Offline tests for roles, authority, proposed actions |
| `profileCli.test.ts` | Offline tests for profile CLI behavior |
| `profiles.test.ts` | Offline tests for profile storage and discovery parsing |

## How to read audit output

Key fields from the TypeScript audit:

| Field | Meaning |
| --- | --- |
| `actorCanApply` | Actor is creator (basic group) or admin with promote rights |
| `actorCanTransferOwnership` | Actor could transfer ownership (precheck only) |
| `targetCanReceiveOwnership` | Target user eligible to receive ownership |
| `proposedActions` | Planned steps (`noop`, `add_user`, `promote_admin`, …) |

If `actorCanApply` is false, the logged-in account cannot change admins in that
chat. For example, auditing a chat from a non-owner session shows false when a
different user is the creator.

## Suggested workflow

1. Run command `tg-sai profile find` to inspect available Telegram
   credential sources.
2. Run command `tg-sai profile add --qr` to save a local profile and generate a
   missing session string.
3. Run command `tg-sai audit --chat <id>` before writes.
4. Run command `tg-sai add-bot --chat <id>` or command
   `tg-sai add-owner --chat <id> --target-user <user>` as a dry-run.
5. Add flag `--run` after reviewing the proposed action.
6. Set environment variable `TELEGRAM_PASSWORD` only for owner transfer runs.

## Why not MCP `get_admins`

MCP `get_admins` uses the supergroup admin filter even on basic groups. That can
list regular members as admins. This tool uses per-member lookups
(`getChatMember` / permissions / full chat info), which match Telegram UI roles.

## Limits

- Bot and owner add/promote execution is wired only for one explicit chat at a
  time.
- Basic groups: admin is on/off only.
- Supergroups/channels: granular admin rights are hard-coded in command
  `add-bot` and command `add-owner`; the named rights policy still needs review.
- Batch audit and batch execution are not implemented.
- Official Sai channel (`saidotfun`) may need a separate policy review.

## Roadmap

- Add batch audit input from file
  `epics/comms/telegram-mcp-sai-target-candidates.md` or a generated JSON file.
- Emit a reviewable table or JSON artifact for batch dry-runs.
- Add named rights profiles for bot admin and owner-target admin promotion.
- Add fixtures for a completed ownership transfer.

## Custody Chat Provisioning

SaiTeam_bot's private `/newdealchat` flow submits an approved request to the
Deal Desk API. A separate worker creates the private supergroup as custody
user `8274304175`, profile `SaiTeam_AI`. The bot, Unique, and the requester
become admins. August and Erick are ordinary default members. The deployed bot
configuration controls the requester allowlist and defaults.

When adding an operator, configure their numeric Telegram user ID in the bot
allowlist. A fresh custody session may not know that ID's access hash. Add a
verified handle alias in `provisioningRuntime.ts` for new team operators and
deploy the worker with it. Resolve the handle through the custody account first
and verify the exact numeric ID and that it is a user account. The runtime
rejects handles that later resolve to a different account.

For explicit CLI requests, prepare a JSON file with a stable `requestId`,
`custodyProfile`, `expectedUserId`, `title`, `participants`, `admins`,
`botRightsProfile`, and `dealIds`. Each admin uses `rightsProfile: "team"`;
the bot uses `botRightsProfile: "baseline"`. `optionalParticipants` must be a
subset of ordinary participants. `dealIds: []` registers a chat without
creating or linking a commercial deal.

```sh
tg-sai chat provision prepare --file request.json
tg-sai chat provision prepare --file request.json --run
tg-sai chat provision run --request-id <request-id>
tg-sai chat provision run --request-id <request-id> --run
tg-sai chat provision status --request-id <request-id>
```

The cloud CLI credential needs the corresponding chat workflow capabilities.
The background worker uses a dedicated read-only record credential with only
`chats:work` granted for workflow writes. The worker service gets only the
custody profile, without personal sessions or ownership-transfer passwords.

```sh
tg-sai worker --custody-profile SaiTeam_AI --once
tg-sai worker --custody-profile SaiTeam_AI --once --run
tg-sai worker --custody-profile SaiTeam_AI --run --interval-ms 5000
```

Without `--run`, the worker reads one queue page and does not connect to
Telegram. Execution uses a durable worker identity, account lock, and local
receipts. Never delete these files to force a retry. An uncertain creation
blocks further creation until an operator reconciles the original request.
Use `chat provision resolve --help` to adopt a verified group with its original
creation marker. Known-group partial setup resumes against that same group.

Required admins must be members before setup becomes ready. An ordinary member
blocked by privacy settings can remain pending with a 24-hour, approval-required
invite link. A link does not mean the person joined. Handle-based resolution of
fixed team defaults must verify the expected numeric ID before any invitation.

The source bot configuration starts with admission in `observe` mode. Check
`~/ki/boku/epics/comms/telegram-bot.md` and its dated rollout evidence for
the actual deployed state before testing bot commands.
