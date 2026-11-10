# Sai Telegram admin bootstrap runbook

Use this when Claude web + MCP is not enough. MCP admin tools can mis-report
roles on **Group (Basic)** chats. This folder holds a TypeScript + mtcute tool
that uses the correct Telegram API paths and keeps decision logic testable
offline.

Location: `jiyuu/ts-pkg/tg-sai/`

## What the tool does today

Command `tg-sai` exposes a Commander CLI for one selected Telegram chat at
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
5. Command `tg-sai add-owner --run` transfers ownership only when a 2FA
   password is available (see section Credentials) and the target user is
   eligible.
6. Command `tg-sai promote-admin --run` promotes a team member who is already
   in the chat. It refuses non-members and bots, and never transfers ownership.

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
with environment variable `TG_SAI_DIR`. The mtcute client cache for each
session lives in directory `~/.tg-sai/sessions/`, one file per session string,
unless a caller passes its own storage path. Runs never write session files
into the working directory. A rotated or revoked session leaves its old cache
file behind; delete it from `~/.tg-sai/sessions/`.

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

Ownership transfer needs the Telegram 2FA password. A profile keeps it in
[`pass`](https://www.passwordstore.org/): field `passwordPassEntry` in file
`~/.tg-sai/profiles.json` names the pass entry, and tg-sai runs `pass show
<entry>` only when Telegram asks for the password during a transfer. The first
line of the entry is the password. A `--run` that may transfer ownership
(`add-owner`, `ops sai-bootstrap`) reads it once before its first change, so a
locked GPG agent stops the command before it promotes anyone. Dry runs,
audits, and the custody worker never read it, so a host without `pass` can
still run them.

The password comes from the same source as the session: the profile's pass
entry for a profile session, the same block's `TELEGRAM_PASSWORD` for a `.env`
or MCP session, and none for a `TELEGRAM_*` environment session. Environment
variable `TELEGRAM_PASSWORD` still overrides for one command, but prefer the
pass entry. A blank password counts as none, so ownership transfer is skipped.

Profiles saved before pass support may hold a plain-text `password` field.
tg-sai still reads it, while `tg-sai profile show` and `tg-sai health` warn
about it. Move it with `tg-sai profile migrate-password` (section Move a
Plain-Text Password to pass).
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

Store the Telegram 2FA password in pass for the same profile when ownership
transfer is needed. The hidden prompt saves it to pass entry
`tg-sai/sai-custody/telegram-2fa` and records only that name in the profile:

```bash
tg-sai profile add --name sai-custody --password-prompt
```

Scripts can pipe the password instead, or point at an entry that already
exists:

```bash
pass show other/entry | tg-sai profile add --name sai-custody --password-stdin
tg-sai profile add --name sai-custody --password-pass-entry other/entry
```

Flag `--password-pass-entry` also names the entry that `--password-prompt` or
`--password-stdin` writes. tg-sai checks that `pass` is installed and
initialised before it starts a login, and a password flag combined with a
login needs `--name` or `--password-pass-entry` so the entry is checked first.
It refuses to write to an existing entry other than the profile's own (the
recorded entry, or `tg-sai/<name>/telegram-2fa` when no other profile records
it) without reading it, so it never
replaces or decrypts another secret. To use such an entry, pass
`--password-pass-entry` alone. tg-sai no longer accepts `--password <value>`, and
it does not save a password it finds in the environment or a discovered
source.

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

Command `tg-sai profile add` saves the Telegram 2FA password in pass with flag
`--password-prompt` or `--password-stdin`. Command `tg-sai profile show`
redacts stored `apiHash`, `sessionString`, and `password`, and prints
`passwordPassEntry` and `passwordStorage` (`pass`, `plain_text`, or `none`).

For password or metadata updates, `--name` selects an existing saved profile by
name, handle, numeric user ID, or saved-list index. The update preserves that
profile's account identity and session without importing environment or discovered
credentials. An unknown password-update selector fails instead of creating a
profile. Explicit login/import cannot replace a known profile with a different
account; use a new name for another account, or remove the old profile first.
Flag `--src` selects a credential source numbered by `tg-sai profile find`, not
the saved-profile index printed by `tg-sai profile list`.

### Move a Plain-Text Password to pass

Command `tg-sai profile migrate-password` moves each plain-text `password` in
`profiles.json` to pass entry `tg-sai/<profile>/telegram-2fa`, records the
entry as `passwordPassEntry`, and removes `password` and `TELEGRAM_PASSWORD`
fields from `~/.tg-sai/backups/*.json`. It is a dry run by default and never
runs `pass` without `--run`:

```bash
tg-sai profile migrate-password          # plan
tg-sai profile migrate-password --run    # apply
```

Select one profile and entry with `--profile <selector> --pass-entry <entry>`.
With `--run`, tg-sai writes each pass entry and reads it back before it
rewrites `profiles.json`, and it refuses an existing entry that holds a
different password. Afterward it scans every file under `~/.tg-sai/` for the
moved passwords and prints the path of each file that still holds one (for
example a `.env` with `TELEGRAM_PASSWORD`), exiting 1 until you clean them up.
Each `--run` scans for every password that a profile keeps in pass, so a rerun
keeps reporting a leftover copy; with `--profile`, it reads only that profile's
entry. The scan reads symlinked files through the link and lists symlinked
directories as `check its target`. A symlinked `profiles.json` is rewritten at
its target. A symlinked backup is never rewritten, since its target may belong
to another app; the run lists it and exits 1 so you can check it by hand. Output never includes password values.
Backup rewrites keep every other value as written, including 64-bit IDs.

The scan covers only `~/.tg-sai/`. If you ever ran
`tg-sai profile add --password '<password>'`, the password is also in your
shell history: delete those lines from `~/.zsh_history` or `~/.bash_history`.
After migrating, change the Telegram 2FA password (Settings > Privacy and
Security > Two-Step Verification) and save the new one with
`tg-sai profile add --name <profile> --password-prompt`.

Create a blank draft profile for onboarding:

```bash
tg-sai profile add --name draft
```

Blank or partial profiles are useful while collecting `apiId`, `apiHash`, and
`sessionString`. When the active profile is incomplete and records no account
(no user ID or handle), commands that connect to Telegram fall back to the
`.env` files and MCP discovery. A profile named with `--profile`, or one that
records a user ID or handle, must exist and be complete; the `.env` and MCP
fallback never substitutes another account's session for it. Complete
`TELEGRAM_*` environment variables still win over `--profile`, per the order in
section Credentials. Bun auto-loads `.env` from the current directory, so
running from `ts-pkg/tg-sai` makes a repo-local `.env` the environment source,
including its `TELEGRAM_PASSWORD`.

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

Command `tg-sai ops sai-find` always writes its chat list as a JSONL file
under directory `~/.tg-sai/logs/`, using the next available filename such as
`sai-find-00.jsonl` or `sai-find-01.jsonl`. This file is command output for
`ops sai-bootstrap --file`, not a log. Use flag `--out <path>` to also write a
copy somewhere else.

Dry-run Sai bot and owner-target updates from a reviewed chat file:

```bash
tg-sai ops sai-bootstrap --file ~/.tg-sai/logs/sai-find-00.jsonl
```

Command `tg-sai ops sai-bootstrap` prints one JSON event per line on stdout
and its `telemetry_run_id`; redirect stdout if you want a copy. Its Telegram
calls are in the telemetry database: `tg-sai events show <run_id>`. The
command waits one second between chats by default. Use flag `--delay-ms <ms>` to change that
delay. When a 2FA password is available (a profile pass entry or
`TELEGRAM_PASSWORD`), execution mode attempts ownership transfer by default for
eligible chats.

The command sleeps through flood waits up to five minutes (environment
variable `TG_SAI_FLOOD_MAX_WAIT_MS` overrides). A longer `FLOOD_WAIT_X` or any
`PEER_FLOOD` stops the remaining chats: stdout gets a `flood_stop` event with
the chat, the remaining count, the code, and `resumeAt`, and the command exits
with code 1. A `SLOWMODE_WAIT_X` fails only that chat.

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

Promote an existing member without adding them or transferring ownership.
Omit `--run` for a dry run:

```bash
tg-sai promote-admin \
  --chat <chat-id> \
  --target-user <target-user> \
  --run
```

The session user must own the chat, because only the owner can grant the full
`add-owner` rights, including add-admins. The command refuses a target that is
not already a member, a bot (use `add-bot`), or a user outside the team
accounts in `cfg.ts` `USERS` unless you pass `--allow-non-team`. Both the dry
run and `--run` print the chat title and the resolved target ID, username,
display name, and bot flag; check them before you pass `--run`.

Telegram adds a non-member when it promotes them, so the command reads the
target's membership again just before it writes and refuses if the target
left or resolves to a different user.

Execute add-owner with ownership transfer enabled. The profile's pass entry
supplies the 2FA password:

```bash
tg-sai add-owner \
  --profile sai-custody \
  --chat <chat-id> \
  --target-user <target-user> \
  --run
```

## Logs, Telemetry, and Flood Holds

tg-sai writes no JSONL run logs. Earlier versions wrote
`~/.tg-sai/logs/<command>-NN.jsonl` and `sai-bootstrap-NN.jsonl`; those files
stay readable but nothing new is added. Each process records into SQLite file
`~/.tg-sai/telemetry.db` (`$TG_SAI_DIR/telemetry.db`):

- `run.start` and `run.end` rows with the sanitized argv and exit code.
- `tg.rpc.call`, `tg.rpc.attempt`, and `tg.flood.wait` rows for Telegram
  calls, with summarized request and response shapes.
- App log rows: a `level` (`debug`, `info`, `warn`, `error`), a short fixed
  `msg`, and summarized `attrs`. Their `event` is `log.<subsystem>` or a
  dotted name such as `worker.job.held`.

Read it with `tg-sai events`. Output is TOON; add `--json` for JSON.

```bash
tg-sai events logs --since 24h --level warn --subsystem worker
tg-sai events logs --run <run_id> --limit 200
tg-sai events show <run_id|job_id|request_id>   # calls and log rows
tg-sai events runs
tg-sai events floods --since 7d
tg-sai events stats                             # rows, log rows by level
```

Saved flood waits live in a separate file, `~/.tg-sai/state.db`. Every tg-sai
process checks it before invite, create, resolve, and send calls. While a
hold is active, a CLI command exits with code 75 and one stderr line naming
the held call group and the UTC time to retry. If `state.db` cannot be read,
invite and create calls are blocked, and the command also exits 75. The
hold check keys by the account the session's own `getMe` reports, not by the
profile's saved user ID; until that is known, invite and create calls are
blocked the same way (`TG_SAI_HOLD_ACCOUNT_UNKNOWN`, exit 75).

`ops sai-bootstrap` stops the batch with a `flood_stop` event on any of
these refusals and exits 75. A `PEER_FLOOD` or a Telegram flood wait longer
than the run's threshold also stops it, with exit 1.

Inspect and override holds with `tg-sai holds`. `set` and `clear` are dry
runs without `--run`; `clear --run` on an active hold also needs `--force`.
Each write adds a `holds.set` or `holds.clear` row to `telemetry.db`. A
`--run` refuses a `state.db` owned by another user, so on the custody VM
run the worker's binary as the worker user:

```bash
sudo -u sai-custody-worker env TG_SAI_DIR=/var/lib/sai-custody-worker HOME=/var/lib/sai-custody-worker /opt/sai-custody-worker/current/tg-sai-worker holds list
```

`TG_SAI_TELEMETRY=off` disables `telemetry.db` writes only. Flood holds in
`state.db` still apply, and `tg-sai events` reads whatever was recorded
before.

`state.db` migrations are forward-only. Once a worker applies one, such as
`0005-member-add-uncertain.sql`, an older binary refuses the file, the hold
check reports `TG_SAI_HOLD_STATE_UNAVAILABLE`, and setup work stops. To
undo a release, roll forward to a fixed build; do not reinstall the older
binary against the same `state.db`.

The `awaiting_join` setup state needs Deal Desk migration 19 or later.
Deploy Deal Desk first, then the worker. A worker from before
`awaiting_join` fails its whole tick on any such row, so before
downgrading the worker, run `sai-deal-desk chat-setups recover` on every
`awaiting_join` row (each returns to `configuring`). The `state.db` rule
above still applies to that downgrade. Deal Desk itself rolls back only by
restoring its backup from before migration 19.

## File roles

| File | Role |
| --- | --- |
| `cfg.ts` | Production bot/user IDs and handles |
| `main.ts` | Commander CLI for audit, add-bot, add-owner, and promote-admin |
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
6. Save the 2FA password in pass with
   `tg-sai profile add --name <profile> --password-prompt` before owner
   transfer runs.

## Why not MCP `get_admins`

MCP `get_admins` uses the supergroup admin filter even on basic groups. That can
list regular members as admins. This tool uses per-member lookups
(`getChatMember` / permissions / full chat info), which match Telegram UI roles.

## Limits

- Bot and owner add/promote execution is wired only for one explicit chat at a
  time.
- Basic groups: admin is on/off only.
- Supergroups/channels: granular admin rights are hard-coded in command
  `add-bot`, command `add-owner`, and command `promote-admin`; the named rights
  policy still needs review.
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

Run one worker per custody account. Deal Desk releases the claim while a
setup waits on its retry or on people to join, so any worker can claim it
again once it is due. The record of who was already added or invited
(`addedIds`, `pendingInvitations`, `botAdds`) lives only in the worker's
local journal. A second worker host for the same account would not see it
and could add people again.

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
Use `chat provision resolve --help` to adopt a verified group whose description
names the request (see Chat Description and Setup Ref). Known-group partial setup resumes against that same group.

Required admins must be members before setup becomes ready. An ordinary member
blocked by privacy settings can remain pending with a 24-hour, approval-required
invite link. A link does not mean the person joined. Handle-based resolution of
fixed team defaults must verify the expected numeric ID before any invitation.

Telegram can accept an add before the person shows up in the chat. The worker
reads the member again after 3 seconds. If they are still absent, it creates
a 24-hour, approval-required invitation for them, saves it in the local
receipt with reason `added_not_visible`, and saves the setup as
`awaiting_join` with error `verify_added_member:TG_SAI_MEMBER_NOT_VISIBLE`,
a `nextCheckAt` 15 minutes out, and the pending invitations. Deal Desk
releases the worker claim for `awaiting_join`, so other setups for the
account run meanwhile, and the bot shows the requester who it waits on and
the link. The worker never adds that person again:

- Each resume approves their join request, if one is waiting, and finishes
  once they are in. Someone who taps the link sees "request sent" and waits
  up to 15 minutes plus one worker tick. Only an admin with invite rights can
  approve sooner. During this wait the team admins are usually not promoted
  yet, because promotion runs after every required member is in, so the
  custody account is often the only admin. Check with
  `tg-sai audit --chat <chat-id> --target-user <user-id>` before asking a
  team admin to approve.
- While the setup waits on that person, it does not pause the background
  queue or other setups, and Deal Desk shows no account hold for it.
- A check that comes due under a saved flood hold moves the next check to
  the hold's end (at most 23 h out, never past the invitation's expiry),
  so Deal Desk does not show the setup as overdue during the hold.
- If the invitation expires first, the setup is saved with error
  `verify_added_member:TG_SAI_INVITATION_EXPIRED` and no `retryAt`, so the
  worker reports `operator_recovery_required`. This holds even on a run
  whose read shows the person while the listing still misses them: the
  worker never replaces an expired `added_not_visible` link. The worker never adds that
  person again, and every resume stops with the same code until they are in.
  To get them in:
  1. If they tapped the link before it expired, their request may still be
     pending. A resume (`sai-deal-desk chat-setups recover`) approves it.
  2. Otherwise, an already-promoted team admin can add them or send a link
     from their own account. The add does not come from the custody budget.
  3. No tg-sai command issues a fresh controlled invitation yet. An add from
     the custody account spends the custody add budget, so Unique decides.
  Once they are in, run `chat-setups recover` to finish the setup.
- An approval error is saved as `approve_join:<code>`. A flood on the
  approval becomes a saved hold and sets its own `retryAt`.

While the setup is `awaiting_join`, Deal Desk has the links too
(`chat-setups view <id>`, field `receipt.pendingInvitations`). After the
setup ends, the link is only in the receipt on the custody host, named by
the SHA-256 of the request ID:

```bash
id='<request id>'
f="/var/lib/sai-custody-worker/provisioning/receipts/$(printf '%s' "$id" | sha256sum | cut -d' ' -f1).json"
sudo jq '.pendingInvitations' "$f"
```

The bot gets no link: a bot that is not visible saves
`TG_SAI_BOT_NOT_VISIBLE` with a 15-minute `retryAt`, and the resumed job adds
it again only if it is still absent. That wait pauses the background queue,
because the setup still owes an add. The receipt counts bot add calls sent
(`botAdds`); a call Telegram refused as a whole, such as a flood, is not
counted. Once two calls were sent and the bot is still absent, the setup
saves `verify_added_member:TG_SAI_BOT_ADDS_EXHAUSTED` with no `retryAt`. The
background queue also reads each member before adding and approves a waiting
join request, so a member who uses the shared link costs no add. A flood
or hold on that read or approval defers the row until it ends. A chat or
member fault there, such as `CHANNEL_PRIVATE`, `PEER_ID_INVALID`, or
`USER_CHANNELS_TOO_MUCH`, fails the row. Any other error counts an attempt
and moves the row behind the others.

A required person whose add is refused for privacy gets the same kind of
invitation, with receipt reason `privacy_restricted`. The setup stays
`awaiting_join` with `add_member:TG_SAI_AWAITING_JOIN` and is checked again
every 15 minutes. Send the person the link from the receipt. The rules above
apply: the worker approves their join request, never adds them again, and stops
for an operator once the invitation expires.

### Chat Description and Setup Ref

Telegram groups have no hidden metadata, so the chat description is how the
worker recognizes a chat it created. It reads the description to find the
chat again after a create call times out (`creation_unknown` candidates), and
checks it before every later change: the ready re-check, each resume, and
`chat provision resolve`. Every check also requires that the custody account
created the chat, as a non-forum supergroup.

New chats get the description `Chat with the Sai team. Ref: <ref>`, where
`<ref>` is the first 8 hex characters of the setup's request fingerprint. The
text lives in `CHAT_DESCRIPTION_TEXT` in `provisioning.ts`; the checks read
only the trailing `Ref: <ref>`, so the wording can change without breaking
existing chats. Chats created earlier carry the legacy marker
`sai-setup:<full fingerprint>`, and the checks still accept it.

Never edit a deal chat's description by hand. A description that names
neither the setup's ref nor its legacy marker fails the checks, and the setup
stops with `Ready setup owner or marker changed` or `verify_marker`. To move
a legacy chat to the new description, use the rewrite command:

```sh
tg-sai chat provision rewrite-description --request-id <request-id>
tg-sai chat provision rewrite-description --request-id <request-id> --run
```

It reads the setup from Deal Desk and the chat ID from the setup, or from the
local receipt when the setup has none. It refuses when the receipt conflicts
with the setup, when the custody account did not create the chat, or when the
current description is anything other than this setup's exact legacy marker.
Both forms connect to Telegram as the custody account and print the chat
title, current description, and new description. The dry run only reads; with
`--run`, it makes one description edit (no flood sleep, no resend; not a
member add) and reads it back. A chat that already has the new description is
reported with `changed: false`. The command takes the custody account lock,
so run it on the custody host while the worker is stopped or idle.

### Setup Failure Paths

A setup left `incomplete` with a `retryAt` resumes on its own. From
migration 19, Deal Desk releases its worker claim on that save, so other
setups for the account run until the retry is due; the worker claims it
again before it resumes or saves a longer hold. An older Deal Desk keeps
the claim until then. One without a `retryAt` waits for an operator. Resumed jobs read each member before
adding, so a retry never adds someone who is already in the chat. The
receipt also lists everyone an add call may have reached (`addedIds`). It
is written before each call is sent and taken back only when Telegram
refused the call (a flood, a hold, or a per-user 400). A resume that finds
one of them absent gives them an `added_not_visible` link instead of
adding them again, whatever step the earlier run stopped at.

A timed-out add means the person joins by link. When a batch add times out,
everyone in it counts as possibly added, so each required person still
absent gets a link, even if Telegram never ran the batch. This trades a
link for the requester and team admins against any second add, in line
with the links-first direction. Only the bot, which cannot use a link, is
added once more.

| Error | Meaning | Retry |
| --- | --- | --- |
| `FLOOD_WAIT_N`, `PEER_FLOOD`, `TG_SAI_HOLD_N` | Account flood limit or saved hold | When the hold ends |
| `TG_SAI_TRANSIENT` | Timeout, dropped connection, Telegram server error, worker shutdown | 5 min; at most 3 add-call failures, or 12 runs stopped at another step |
| `TG_SAI_TRANSIENT_LIMIT` | `add_member:` after the third add-call failure; any other step after 12 runs in a row stopped there | Operator |
| `TG_SAI_HOLD_STATE_UNAVAILABLE`, `TG_SAI_HOLD_ACCOUNT_UNKNOWN` | Local `state.db` or session account not readable | 5 min |
| `TG_SAI_QUEUE_UNAVAILABLE` | Writing optional members to the local queue failed | 5 min; counts as a stall |
| `TG_SAI_MEMBER_NOT_VISIBLE` | Added or promoted, but not shown in the chat yet; the person gets an `added_not_visible` link | 15 min |
| `TG_SAI_BOT_NOT_VISIBLE` | Bot added but not shown yet | 15 min, one re-add |
| `TG_SAI_AWAITING_JOIN` | Waiting for a privacy-restricted person to use their link | 15 min |
| `TG_SAI_INVITATION_EXPIRED` | An invited person never joined | Operator |
| `TG_SAI_BOT_ADDS_EXHAUSTED` | Bot still absent after two add calls | Operator |
| `verify_marker:*` (not transient), `audit_membership:provisioning_step_failed`, `CHAT_ADMIN_REQUIRED`, `RIGHT_FORBIDDEN`, other `USER_*` refusals | Chat ownership, rights, or the person's account | Operator |

A timeout during group creation is different: the group may exist, so the
setup becomes `creation_unknown` and needs `chat provision resolve`.

An add call that timed out or got a Telegram server error may still have
invited the user, so it counts toward today's adds (`member_add` outcome
`uncertain`) and the background daily cap. In the background queue it also
counts as an attempt. mtcute does not resend create, add, or approval calls
after a server error (`PROVISION_WRITE_PARAMS`); the error reaches these
counters instead. A background add cut off by a worker restart also counts
toward today's adds, but not as an attempt.

Two counts lead to `TG_SAI_TRANSIENT_LIMIT`:

- Add calls (`transientRetries`): the third transient failure stops the
  setup. The count resets once a run gets past the adds.
- Every other step (`stalledRuns`): reads, approvals, marker checks,
  promotions, the final listing, and link creation spend no add, but each
  retry keeps the account busy in Deal Desk and pauses the background
  queue. A failed local queue write counts too, and so does a person or
  the bot that a read shows but the final listing or a promotion keeps
  missing. A person gets one `added_not_visible` link per setup; no run
  mints a second. The receipt keeps the furthest step a counted stall
  reached (`stalledStep`). Every stall at or before it counts; the
  twelfth, about an hour, stops the setup. A transient stop beyond that
  step starts the count over, and so does any stop that is not transient,
  such as a link wait (Telegram answered every call up to it). A flood or
  hold leaves the count alone.

A run cut short by worker shutdown counts toward neither. Reads, the
identity checks, and promotions resend after a Telegram server error at
most three times and give up after 30 s (`PROVISION_READ_PARAMS`), so a
persistent server error reaches these counts instead of looping inside one
call. Creating a controlled invitation is never resent
(`PROVISION_LINK_PARAMS`), since each resend could mint another link.

These limits are counters in the receipt (`botAdds`, `transientRetries`,
`stalledRuns`). The worker resets the transient ones only as described
above, and Deal Desk recovery does not touch the receipt. To recover:

1. `TG_SAI_BOT_ADDS_EXHAUSTED`: check the chat with
   `tg-sai audit --chat <chat-id> --target-user 8983431101`. If the bot is
   absent, have a team admin add it, or decide to spend one more custody add
   by clearing the counter below. Then run `chat-setups recover`.
2. `TG_SAI_TRANSIENT_LIMIT`: check that Telegram and the worker host are
   healthy. For `add_member:` clear `transientRetries`; for any other step
   clear `stalledRuns` and `stalledStep`. Then run `chat-setups recover`.

Clear a counter with the worker stopped. `$f` is the receipt path from the
lookup above:

```bash
sudo sh -c 'jq "del(.botAdds)" "$1" > "$1.new" &&
  chown --reference="$1" "$1.new" && chmod 600 "$1.new" &&
  mv "$1.new" "$1"' _ "$f"
```

The new file keeps the receipt's owner and mode. Use
`del(.transientRetries)` for the transient limit. The worker reads the
receipt again on the next run.

New ready receipts include an `openGroupUrl` anchored to a message actually
observed in the group and the existing `primaryInviteLink`, when available.
These read-only lookups run together with a 10-second limit each. A missing link
does not undo verified setup. Existing primary invitations are never rotated or
created by this step, and privacy-blocked members keep their separate controlled
invitations. The bot displays the saved links in its completion reply and
`/chatstatus`. Completed requests are not rewritten to refresh missing links.

The source bot configuration starts with admission in `observe` mode. Check
`~/ki/boku/epics/comms/telegram-bot.md` and its dated rollout evidence for
the actual deployed state before testing bot commands.


## Runtime Health and Deployment Readiness

`tg-sai health` now defaults to offline runtime checks. It verifies both bundled
WASM assets, crypto and compression operations, and actual Telegram client
preparation with a temporary SQLite store. It does not read profiles or contact
Telegram. `--json` provides a machine-readable result and failures exit nonzero.

```sh
tg-sai health --offline --json
tg-sai health --online --custody-profile SaiTeam_AI --json
```

Online mode additionally strict-loads custody user `8274304175`, acquires the
same account lock used by provisioning, and performs bounded `getMe` validation.
It cannot fall back to a personal account and does not create or modify chats.
An account-lock conflict fails the check; run it while the worker is stopped or
idle. The earlier active-profile/workstation diagnostics remain available with
`tg-sai health --local` and are separate from custody-worker readiness.

The deployed executable includes a worker command and the same health commands.
A release candidate must pass offline health as the worker Unix user before
installation selects it. The worker systemd unit runs offline and online checks
before starting the queue loop. A service being alive, `--help`, or an empty
queue preview does not prove Telegram client initialization.

From the Boku Deal Desk source directory, `just bot-health` checks the deployed
runtime under its service sandbox. `just bot-health --online` also validates
custody identity. These are operator probes, not the API's `/health` endpoint.
That endpoint reports API health and whether bot ingress is configured; it does
not certify Telegram custody readiness.
