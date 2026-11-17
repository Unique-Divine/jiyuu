# Telegram MCP

<!-- mcp-name: io.github.unique-divine/telegram-mcp -->

Read and manage Telegram through an authenticated user account. This Python
server supports messages, chats, participants, folders and media over MCP stdio.

Modified by Unique Divine in 2026 for Jiyuu installation, connection recovery
and participant pagination. Original source and license: [IMPORT.json](IMPORT.json)
and [LICENSE](LICENSE).

## Install

Requirements: [uv](https://docs.astral.sh/uv/) and Telegram credentials (see
[Authenticate](#authenticate)). Run the published package with an exact
version pin:

```sh
uvx uniquedivine-telegram-mcp@2.0.1
```

The PyPI name is `uniquedivine-telegram-mcp`. Do not install the unrelated
package named `telegram-mcp` from PyPI. CI in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu) builds and
publishes this package to PyPI from this directory when its version changes
on `main`. The pin decides which reviewed version runs.

Generate a session string with the same pinned package:

```sh
TELEGRAM_API_ID=123456 TELEGRAM_API_HASH=your_api_hash \
  uvx --from uniquedivine-telegram-mcp@2.0.1 telegram-mcp-generate-session --qr
```

The examples below pass credentials as environment variables. Keep the real
values out of version control.

### Claude Code

```sh
claude mcp add telegram \
  --env TELEGRAM_API_ID=123456 \
  --env TELEGRAM_API_HASH=your_api_hash \
  --env TELEGRAM_SESSION_STRING=your_session_string \
  -- uvx uniquedivine-telegram-mcp@2.0.1
```

### Codex

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.telegram]
command = "uvx"
args = ["uniquedivine-telegram-mcp@2.0.1"]

[mcp_servers.telegram.env]
TELEGRAM_API_ID = "123456"
TELEGRAM_API_HASH = "your_api_hash"
TELEGRAM_SESSION_STRING = "your_session_string"
```

### Cursor

Add to `~/.cursor/mcp.json`, or `.cursor/mcp.json` in a project:

```json
{
  "mcpServers": {
    "telegram": {
      "command": "uvx",
      "args": ["uniquedivine-telegram-mcp@2.0.1"],
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

1. Find the newest version at
   https://pypi.org/project/uniquedivine-telegram-mcp/ or with
   `curl -s https://pypi.org/pypi/uniquedivine-telegram-mcp/json | jq -r .info.version`.
2. Change the `@2.0.1` pin in the client config to that version.
3. Restart the MCP server in the client (for example `/mcp` in Claude Code)
   or restart the client. `uvx` installs the new version on the next launch.

## Install From Source

Requirements: Git, uv and Python 3.13. The source-install guard remains
enabled: it refuses to start when the unrelated `telegram-mcp` distribution is
installed in the same environment.

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
2.0.1; `[tool.jiyuu.upstream]` in `pyproject.toml` records that baseline.
