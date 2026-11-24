# Sai CLI

Use this page to run `sai` commands. For trades, also read
[trading-safety.md](trading-safety.md). When Sai MCP tools are available,
prefer them and read [mcp.md](mcp.md).

Command `sai --help` lists the command groups: `account`, `auth`, `config`,
`doctor`, `intent`, `logs`, `market`, `mcp`, `trade`, `update`, and `watch`,
plus `skill` on versions that have it. Global options go before the command:

```bash
sai --json market prices
sai --as 0xOTHER_ADDR --json account positions 0xOTHER_ADDR
```

`--json` prints compact JSON. `--as ADDR` runs one command as another
connected account. `sai` with no arguments opens an interactive shell, which
an agent has no use for.

## Read Markets and Accounts

```bash
sai --json market list
sai --json market prices
sai --json account positions 0xYOUR_ADDR
sai --json account balances 0xYOUR_ADDR
sai --json account history 0xYOUR_ADDR
sai --json watch price 3
```

- `market list` returns Keeper's borrowing markets, and `market prices` the
  current indexed prices.
- `account positions ADDR` lists positions and pending orders.
  `account balances ADDR` reads one balance snapshot. `account history
  ADDR` reads trade history. These reads need no sign-in.
- `watch price TOKEN_ID`, `watch positions ADDR`, and `watch balances
  ADDR` stream JSON lines until interrupted. A stream reconnects after a
  transport interruption and emits a fresh snapshot.

The CLI does not deposit or bridge funds. The person funds the Sai account in
Sai Web.

## Connect an Account

Trading needs a Sai session. When the person asks you to connect Sai:

1. Ask them to open the wallet menu on Sai Web Mainnet and select **Connect
   Sai CLI / MCP** beside **Send to phone**. The button appears for the
   selected, connected wallet with an active trading session. Ask them to
   check the expiry and click **Copy session token**. If they can't use the
   button, they can sign in with a Foundry keystore as described below.
2. Run `sai --json --endpoint https://sai-keeper.nibiru.fi/query auth import`
   without a terminal. The command reads the clipboard, stores the token,
   and prints `addr`, `expiresAt`, and `endpoint`. It tries to clear the
   clipboard if it still holds the token.
3. If your commands run in a pseudo-terminal, `auth import` shows a hidden
   prompt instead. Send only an empty line, which makes it read the
   clipboard.
4. If it reports that no clipboard command works, as over SSH, ask the person
   to run the dialog's `sai auth import` command in their own terminal. They
   can paste the token at the hidden prompt or press Enter to read the
   clipboard.

Never ask the person to paste the token into the chat. `auth import` does not
accept the token as an argument, environment variable, or piped input.

The import command pins Mainnet for that invocation only. Check the session
with `sai --endpoint https://sai-keeper.nibiru.fi/query auth status`. Later CLI
commands and any MCP server using the session must target the same Keeper
endpoint. To save Mainnet as the default, run
`sai config set keeper.endpoint https://sai-keeper.nibiru.fi/query`.
Update any conflicting `--endpoint` or `SAI_ENDPOINT` overrides and restart
running MCP servers to pick up the change.

A person with a Foundry keystore can run `sai auth login NAME` in their own
terminal instead. `NAME` is a keystore in `~/.foundry/keystores`, and `cast`
prompts for its password. `--addr ADDR` is an optional cross-check that stops
sign-in when the keystore holds another account. Leave it out unless the
person gives one.

Once connected, commands need no prompt until the session expires.

| Command | Use |
| --- | --- |
| `sai auth status` | The active account's addr and expiry. Never prints the token. |
| `sai auth list` | Every connected account, its expiry, and which is active. |
| `sai auth use ADDR` | Switch the active account. Only when the person asks. |
| `sai --as ADDR ...` | Run one command as another connected account. |
| `sai auth logout [ADDR]` | Remove one account, by default the active one. |

Each sign-in adds an account and makes it active. If the person asks what the
token is, tell them:

- It is the same session token Sai Web keeps in the browser.
- It lasts as long as Keeper issues it, 30 days in production.
- Anyone with the token can trade with their Sai Balance and request
  one-click withdrawals of idle funds. Those withdrawals can only go to
  the wallet that owns the session. Keep the token out of clipboard sync,
  chats, and shared notes.
- Keeper cannot revoke it. `sai auth logout` deletes only the local copy, and
  signing out of Sai Web does not sign out the CLI.

The CLI stores sessions in `~/.sai/trader/sessions.json` with mode 0600,
encrypted with a key built into the CLI, and never prompts for a passphrase.
If it reports a passphrase-encrypted session from an older version, ask the
person to sign in again.

## Settings

Settings live in file `~/.sai/config.toml`, or under `$SAI_HOME` when that
variable is set. The MCP server reads the same file.

```bash
sai --json config list
sai config get keeper.endpoint
sai config unset keeper.endpoint
```

| Key | Meaning | Variable | Flag |
| --- | --- | --- | --- |
| `keeper.endpoint` | Keeper GraphQL HTTP endpoint | `SAI_ENDPOINT` | `--endpoint` |
| `keeper.ws_endpoint` | Keeper WebSocket endpoint. Unset derives it from `keeper.endpoint`. | `SAI_WS_ENDPOINT` | `--ws-endpoint` |
| `lcd.endpoint` | Nibiru REST endpoint for margin and partial-close reads | `SAI_LCD_ENDPOINT` | `--lcd-endpoint` |
| `update.auto` | `true` (default): sai installs new releases by itself, about once a day. `false`: it only says a release is available. | `SAI_UPDATE_AUTO` | |
| `logs.enabled` | `true` (default): record commands, tool calls, and requests under `~/.sai/logs`. | `SAI_LOG` | |
| `logs.keep_days` | Days of log files to keep. Default `14`. | `SAI_LOG_KEEP_DAYS` | |

A flag overrides a variable, which overrides the file, which overrides the
mainnet default. `config list` shows each key's value and its source: `flag`,
`env`, `file`, `default`, `derived`, or `invalid`. `invalid` marks a `logs.*`
value sai could not read, which turns logging off. Change settings with `sai config set`
and `sai config unset`. Do not edit the file or an MCP client's config by
hand.

### Switch Networks

The Keeper and LCD endpoints must name the same chain. Margin and
partial-close commands read the Perp contract through the LCD and refuse when
its chain differs from Keeper's. To use Testnet2, set both:

```bash
sai config set keeper.endpoint https://sai-keeper.testnet-2.nibiru.fi/query
sai config set lcd.endpoint https://lcd.testnet-2.nibiru.fi
sai config unset keeper.ws_endpoint
sai --json config list
```

Unsetting `keeper.ws_endpoint` lets the CLI derive it from the Keeper
endpoint. If `config list` shows `keeper.ws_endpoint` or another key with
source `env` or `flag`, that override still wins; tell the person which
variable to change. To return to mainnet, `sai config unset` both keys.

A session is bound to the Keeper endpoint it was issued for. After changing
`keeper.endpoint`, the person signs in again for that network.

## Diagnose With `sai doctor`

When a command fails in a way you cannot explain, run:

```bash
sai --json doctor
```

Doctor checks the install, the session, the intent ledger, and the Keeper
HTTP, WebSocket, and LCD endpoints. It fixes what is safe to fix: file modes
under `$SAI_HOME/trader`, the ledger schema, and unknown intents it can
settle by reading Keeper. It never trades, never rewrites the session, and
never prints the token.

- Each check has a `status` of `pass`, `fixed`, `warn`, `fail`, or `skipped`,
  and a `fix` command when one applies. `changes` lists what doctor changed.
- Show the person what doctor changed, then the checks that still fail and
  their fixes.
- `--no-fix` reports without changing anything. `--offline` skips the network
  checks.
- Doctor exits 1 while any check still fails.

## Logs

sai records what it does as JSON lines under `~/.sai/logs` (`$SAI_HOME/logs`):
each CLI command, each MCP tool call, each Keeper request, and each intent
status change. It never writes the session token. Read the logs when a
command or tool call failed, timed out, or left a trade's outcome unclear.

```bash
sai --json logs tail -n 50
sai --json logs tail -n 50 --surface mcp
sai --json logs tail --event intent
sai --json logs tail --event keeper.request
sai logs path
```

- `tail` shows the most recent events, oldest first. `-n` sets how many.
  With `--json` each line is the stored JSON object.
- `--surface` is `cli`, `mcp`, or `update`. Each surface has its own file
  per UTC day.
- `--event NAME` matches one event or a family by prefix: `intent`,
  `keeper`, `mcp`, `command`, `update`.
- `sai logs path` prints the directory. Tool `mcp_info` reports the same
  directory as `logs.dir`.

| Event | Fields to read |
| --- | --- |
| `command.end` | `exitCode`, `error`, `durationMs` |
| `mcp.tool_result` | `code`, `durationMs`, `error` |
| `keeper.request` | `operation`, `status`, `httpStatus`, `keeperErrors` |
| `intent.status` | `intentKey`, `previous`, `status` |

Every line carries `ts`, `surface`, `event`, and `runId`. `runId` is one id
per process, so it ties the lines of one command together. Lines written
during an MCP tool call also carry `tool` and `callId`; filter on `callId` to
follow one call from `mcp.tool_call` to `mcp.tool_result`. To follow one
trade, filter on its `intentKey`.

A `keeper.request` has `status` `ok`, `error`, `rejected` (Keeper refused
the request), or `not_sent` (the connection failed first). Operation `submit`
is the trade submission.

Logging is on by default. `sai config set logs.enabled false` or `SAI_LOG=0`
turns it off, and `logs.keep_days` sets how long files stay. Do not turn
logging off or delete log files unless the person asks.

## Update

```bash
sai --version
sai --json update --check
sai update
```

- `sai update` installs the latest release over the current install. The
  `sai` path and everything under `~/.sai` stay the same, so MCP
  registrations keep working. If a step fails, the previous version stays.
- `sai update --check` only reports the installed and latest versions.
- sai also updates by itself. With `update.auto` at its default `true`, a
  command a person runs in a terminal starts an update in the background at
  most once a day. With `update.auto` set to `false`, sai only says a release
  is available.
- A command run with `--json`, or without a terminal, never starts an update.
  Commands an agent runs therefore do not update sai by themselves. Run `sai
  update` when the person asks for the update.
- After an update, ask the person to restart or reconnect their agent client.
  A running MCP server keeps the old code until then; tool `mcp_info` reports
  `restartNeeded`.
- `sai update` only updates a `uv tool install` of a release or of the
  official Git source. A checkout, a `uvx` run, or another install gets a
  note and exit code 3. Follow the note.

## This Skill

`sai --skill` prints the `SKILL.md` packaged with the installed CLI. A CLI
that lists `skill` in `sai --help` also has:

```bash
sai skill show --page mcp
sai --json skill source
sai skill install
```

- `sai skill show` prints `SKILL.md`, and `--page NAME` prints one child
  page.
- `sai skill source` reports the packaged path and the commit the skill was
  copied from.
- `sai skill install` copies the whole skill into `~/.agents/skills/sai`, or
  into `--dir DIR`. It refuses to replace a symlink or a copy that differs
  unless given `--force`. Ask the person before using `--force`.

If `sai skill` answers "No such command", the installed CLI predates it. Use
`sai --skill`, and read the child pages in the
[skill's repository](https://github.com/Unique-Divine/jiyuu/tree/main/ai-skills/sai).

## Trade Commands

`trade open`, `trade close`, `trade partial-close`, `trade tpsl`, `trade
leverage`, and `trade margin` submit intents, and `intent local` and `intent
status` follow them. [trading-safety.md](trading-safety.md) has the commands,
the dry-run examples, and the rules.
