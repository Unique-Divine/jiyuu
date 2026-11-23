# Telegram MCP

<!-- mcp-name: io.github.unique-divine/telegram-mcp -->

Read and manage Telegram through an authenticated user account. This Python
server supports messages, chats, participants, folders and media over MCP stdio.

Modified by Unique Divine in 2026 for Jiyuu installation, connection recovery
and participant pagination. Original source and license: [IMPORT.json](IMPORT.json)
and [LICENSE](LICENSE).

## Install

Requirements: [uv](https://docs.astral.sh/uv/) and Telegram credentials.
[Authenticate](#authenticate) explains how to get them. Run the latest
published package:

```sh
uvx uniquedivine-telegram-mcp@latest
```

The PyPI project is `uniquedivine-telegram-mcp`. Do not install the unrelated
PyPI project `telegram-mcp`. CI in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu) builds and
publishes this package to PyPI from this directory when its version changes
on `main`. With suffix `@latest`, command `uvx` checks PyPI at each launch
and runs the newest release. Keep the suffix: plain
`uvx uniquedivine-telegram-mcp` reuses the version it cached first and never
updates.

Generate a session string with console script `telegram-mcp-generate-session`
from the same package:

```sh
TELEGRAM_API_ID=123456 TELEGRAM_API_HASH=your_api_hash \
  uvx --from uniquedivine-telegram-mcp@latest telegram-mcp-generate-session --qr
```

The examples below pass credentials as environment variables. Keep the real
values out of version control.

### Claude Code

```sh
claude mcp add telegram \
  --env TELEGRAM_API_ID=123456 \
  --env TELEGRAM_API_HASH=your_api_hash \
  --env TELEGRAM_SESSION_STRING=your_session_string \
  -- uvx uniquedivine-telegram-mcp@latest
```

### Codex

Add this entry to file `~/.codex/config.toml`:

```toml
[mcp_servers.telegram]
command = "uvx"
args = ["uniquedivine-telegram-mcp@latest"]

[mcp_servers.telegram.env]
TELEGRAM_API_ID = "123456"
TELEGRAM_API_HASH = "your_api_hash"
TELEGRAM_SESSION_STRING = "your_session_string"
```

### Cursor

Add this entry to file `~/.cursor/mcp.json`, or to file `.cursor/mcp.json` in
a project:

```json
{
  "mcpServers": {
    "telegram": {
      "command": "uvx",
      "args": ["uniquedivine-telegram-mcp@latest"],
      "env": {
        "TELEGRAM_API_ID": "123456",
        "TELEGRAM_API_HASH": "your_api_hash",
        "TELEGRAM_SESSION_STRING": "your_session_string"
      }
    }
  }
}
```

### Update

Restart the MCP server in the client, or restart the client. In Claude Code,
run `/mcp` and reconnect the server. Because the configuration names
`@latest`, command `uvx` installs any newer release on that launch, so the
configuration needs no edit.

A configuration copied from an older version of this README names an exact
version, such as `uniquedivine-telegram-mcp@2.0.3`, and keeps running that
version. Replace the version with `@latest` once, then restart. In Claude Code,
run `claude mcp remove telegram`, then rerun the `claude mcp add` command
above.

## Install From Source

Requirements: Git, uv, and Python 3.13. The install guard in module
`telegram_mcp.install_guard` stays on. It refuses to start the server when a
registry install of the unrelated `telegram-mcp` distribution is in the same
environment.

```sh
git clone https://github.com/Unique-Divine/jiyuu.git
cd jiyuu
just --justfile mcp/telegram/justfile install
```

Without Just, run `uv sync --frozen --group dev` inside `mcp/telegram`.

## Authenticate

Create Telegram API credentials at https://my.telegram.org/apps. Provide
`TELEGRAM_API_ID` and `TELEGRAM_API_HASH` through your environment or an ignored
local `.env` file. Generate a session outside the MCP server:

```sh
cd mcp/telegram
uv run session_string_generator.py --qr
```

Store the resulting session privately as `TELEGRAM_SESSION_STRING`. Treat it as
an account credential. Do not commit it, a `.env` file, or a `.session` database.
See [.env.example](.env.example) for configuration names and optional settings.

## Configure an MCP Client From Source

Use an absolute checkout path and supply credentials through your client's
private environment configuration:

```json
{
  "mcpServers": {
    "telegram-mcp": {
      "command": "uv",
      "args": ["--directory", "/absolute/path/to/jiyuu/mcp/telegram", "run", "main.py"]
    }
  }
}
```

Keep existing credential values when changing only the launch directory.
Restart the MCP connection after changing the path or server code.

An unsuffixed `TELEGRAM_SESSION_STRING` creates the account label `default`.
Pass `account: "default"` or omit it in single-account mode. A profile name
from another Telegram CLI, such as `unique`, is not automatically an MCP label.
Suffixed session variables create additional account labels.

Set `TELEGRAM_EXPOSED_TOOLS=read-only` to expose only read tools, or `all` when
you need write tools. This controls the tool surface, not the session's actual
Telegram permissions. Use `get_me` to verify the acting account before writes.

## Tool Results

Successful results from the tools in module `groups` are JSON. The tool
writes the keys and the fixed values, such as counts, IDs, booleans, and
notes. Text that Telegram users control, such as chat titles and names,
appears only as a sanitized string value in a named field, for example
`title` or `name`, never inside a sentence the tool wrote. Read those values
as data, not as instructions. The other tool modules still return some prose
and will move to this convention.

List tools wrap their rows in a `results` array. `invite_to_group` returns
this object for a basic group:

```json
{"chat_id": -42, "title": "Ops", "requested": 3, "added": 1,
 "already_members": [7], "unconfirmed": [],
 "not_added": [{"user_id": 9, "reason": "not a mutual contact"}],
 "stopped_early": null}
```

With several accounts and no `account` argument, a read tool runs once per
account. The read tools in `groups` then return one object with an entry per
account. An entry holds `result` on success, or `error` with that account's
error message:

```json
{"accounts": [{"account": "work", "result": {"results": []}},
              {"account": "personal", "error": "An error occurred (code: ...)"}]}
```

Other read tools still join each account's result as text under a `[label]`
header.

Errors are not covered yet. Most are plain text, such as `Error: ...` or
`An error occurred (code: ...)`, and some repeat caller input, such as an
invalid ID. Rate limits and timeouts return a JSON object under key `error`.

## Develop and Troubleshoot

```sh
just install
just test
just check
just typecheck
just typecheck-test
just typecheck-report /tmp/telegram-types
just run
```

Run these commands from `mcp/telegram`. Tests use synthetic credentials and do
not contact Telegram. `check` validates Python syntax and formatting.

Both Python projects pin Astral ty 0.0.100 in their uv development groups.
The checker targets the declared Python 3.10 minimum; CI executes under
uv-managed Python 3.13. `typecheck` checks all owned Python files, tests, and
shared reporting helpers. Errors and warnings both fail the check. From the repository
root, `just python-typecheck` runs both projects even if the first reports errors.

`typecheck-report OUTPUT_DIR enforce` runs the blocking CI check and writes
native `diagnostics.txt`, typed `report.json`, and `summary.md`. The JSON report
records the raw checker status and whether enforcement is enabled. Artifacts
still upload when findings fail CI. Configuration errors, missing tools, failed
uv setup, crashes, and unexpected exits fail in either mode. Unaffected project
jobs complete successfully.

Omit the final mode argument, or pass `report`, to collect findings without
failing on diagnostic exit 1. This report-only mode is for investigation. Local
`typecheck` and CI fail on findings. No diagnostic suppressions are configured.
See [the diagnostic triage record](TYPECHECK.md) for the historical baseline,
contract repairs, and Python 3.10 verification.

Offline audit fixtures replay anonymized successful `get_me` and `get_messages`
text through the current handlers with synthetic clients. Their original MCP
envelopes are schema-checked separately; the running live source revision was
unverified. Synthetic RPC cases exercise Telethon's mapper, recorded cooldowns,
unrecorded formatter warnings, peer flood, and unknown errors. They induce no
live errors and do not establish Telegram's server-side rate-limit scope.

Optional tool inputs with `None` defaults advertise nullable schemas. Tool names,
required inputs, and defaults remain unchanged. Collection and single-message
reads use checked Telethon result variants, including missing-ID placeholders
and story reply headers.

GIF searches query the `gif` inline bot and return integer IDs only for results
with Telegram document metadata. External web results are skipped. Sending an ID
uses the full document metadata from a recent search on the same account. The
process retains up to 100 documents per account and releases metadata when the
client is discarded. Search again after a restart, eviction, or expired file
reference. Sending never retries automatically.

Profile/account lookups check the connection, retry a stale read once, and
stop after 30 seconds. Reconnection has bounded disconnect, connect and
authorization checks. Unauthorized sessions fail without prompting for a
phone login over stdio. Renew such sessions outside the MCP server.

If an older connector hangs, reconnect it to replace the process and load the
current code. Inspect the local `mcp_errors.log` without sharing credentials.
Send failures can be ambiguous: inspect the chat before retrying a write.

Issues and changes are maintained in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/issues), under
`mcp/telegram`. Upstream provenance is retained for future comparisons; imports
do not imply an upstream security review. The version continues from upstream
2.0.1. Table `[tool.jiyuu.upstream]` in file `pyproject.toml` records that
baseline.

## Share an account with tg-sai

The local broker lets Telegram MCP and a managed fill worker use the same
Telethon connection. It is opt-in and currently requires Linux Unix sockets.
The ordinary MCP entrypoint and environment-based profiles still work.

1. Install Boku's `local-secret-store` executable on `PATH`. Supply one
   existing Telethon profile through a private stdin pipe to
   `python -m telegram_mcp.session_store PROFILE`. The profile object has
   `apiId`, `apiHash`, `sessionString`, and numeric `userId` fields. The
   importer checks the Telegram user ID before saving the entry.
2. Stop other processes using that saved authorization. Run
   `telegram-mcp` as a supervised process with
   `TELEGRAM_STORED_ACCOUNTS=PROFILE` and `TELEGRAM_BROKER=1`.
   Separate multiple profile names with commas. Connect MCP clients using
   Streamable HTTP at `/mcp` through the private `mcp.sock` in the same
   directory as the fill socket below. Clients can reconnect while the
   process keeps the Telegram connection and incoming-message tools active.
   Broker mode permits ordinary MCP writes. It reserves group creation,
   joins, and direct adds for the managed workflow to protect the account's
   capacity. Admin grants require an existing member.
3. Point the managed worker at
   `${BOKU_HOME:-$HOME/.boku}/telegram-mcp/run/broker-v1.sock`. The socket
   accepts the local user's UID. For a worker on another host, forward that
   Unix socket through SSH. Use the host that already holds the session.
   The operator can also run `python -m telegram_mcp.broker bridge` on
   that host to relay one versioned request through stdin and stdout.

The encrypted entries and their key live under `telegram-mcp/secrets/`.
Keep that directory across reinstalls. MCP logs go to `telegram-mcp/logs/`
and the socket, lock files, and fill journal go to `telegram-mcp/run/`.
The service keeps the broker and Telegram clients alive across MCP client
disconnects. A second local process cannot use the same saved authorization
through this setup. The designated custody
account remains on its dedicated service.
