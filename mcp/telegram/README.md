# Telegram MCP

Read and manage Telegram through an authenticated user account. This Python
server supports messages, chats, participants, folders and media over MCP stdio.

Modified by Unique Divine in 2026 for Jiyuu installation, connection recovery
and participant pagination. Original source and license: [IMPORT.json](IMPORT.json)
and [LICENSE](LICENSE).

## Install

Requirements: Git, [uv](https://docs.astral.sh/uv/) and Python 3.13. Run from a
source checkout; do not install the unrelated package named `telegram-mcp` from
PyPI. The source-install guard remains enabled.

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

## Configure an MCP Client

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
shared reporting helpers, and preserves ty's failure status. From the repository
root, `just python-typecheck` runs both projects even if the first reports errors.

`typecheck-report` is the same recipe CI uses after `just install`. It writes
native `diagnostics.txt`, typed `report.json`, and `summary.md` into the given
directory. Existing diagnostics are initially report-only in CI. Only the direct
checker's diagnostic exit 1 becomes success. Configuration errors, missing tools,
failed uv setup, crashes, and unexpected exits fail. CI publishes the actual
counts and raw checker status, and completes unaffected project jobs successfully.
No diagnostic suppressions are configured.

Offline audit fixtures replay anonymized successful `get_me` and `get_messages`
text through the current handlers with synthetic clients. Their original MCP
envelopes are schema-checked separately; the running live source revision was
unverified. Synthetic RPC cases exercise Telethon's mapper, recorded cooldowns,
unrecorded formatter warnings, peer flood, and unknown errors. They induce no
live errors and do not establish Telegram's server-side rate-limit scope.

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
do not imply a new package release or an upstream security review.
