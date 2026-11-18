# Slack MCP

Read Slack channels, DMs, threads, and search results through your own
browser session, and send, edit, or delete your messages when you ask. The
server needs no Slack app and no admin approval.

Maintained by [Unique Divine](https://uniquedivine.com) in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu). Forked from
[jtalk22/slack-mcp-server](https://github.com/jtalk22/slack-mcp-server); see
[Attribution and License](#attribution-and-license).

## Install

Requirements: [Bun](https://bun.sh) 1.4.3 or newer and a Slack account signed
in through a browser. Store credentials once, then run the published package
with an exact version pin:

```sh
bunx @uniquedivine/mcp-slack@4.5.2 --setup
bunx @uniquedivine/mcp-slack@4.5.2
```

Flag `--setup` writes file `~/.slack-mcp-tokens.json`. To pass environment
variables `SLACK_TOKEN` and `SLACK_COOKIE` instead, see
[Authenticate](#authenticate). CI in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu) builds and
publishes `@uniquedivine/mcp-slack` to npm from this directory when its version
changes on `main`. The version pin selects the exact reviewed release that runs.

### Claude Code

```sh
claude mcp add slack -- bunx @uniquedivine/mcp-slack@4.5.2
```

### Codex

Add this entry to file `~/.codex/config.toml`:

```toml
[mcp_servers.slack]
command = "bunx"
args = ["@uniquedivine/mcp-slack@4.5.2"]
```

### Cursor

Add this entry to file `~/.cursor/mcp.json`, or to file `.cursor/mcp.json` in
a project:

```json
{
  "mcpServers": {
    "slack": {
      "command": "bunx",
      "args": ["@uniquedivine/mcp-slack@4.5.2"]
    }
  }
}
```

### Update

1. Find the newest version with command
   `npm view @uniquedivine/mcp-slack version`.
2. Change the `@4.5.2` pin in the client configuration to that version.
3. Restart the MCP server in the client, or restart the client. In Claude
   Code, run `/mcp` and reconnect the server. Command `bunx` downloads the new
   version on the next launch.

The token store and workflow files live in your home directory, so an update
needs no new credentials. Tool `mcp_info` reports the running version and
whether a newer release exists.

## Tools

The server has 25 Slack tools. The stdio server also lists tool `mcp_info`.
[docs/API.md](docs/API.md) documents parameters, results, and errors.

| Group | Tools |
|-------|-------|
| Session | `slack_health_check`, `slack_token_status`, `slack_refresh_tokens` |
| Read | `slack_list_conversations`, `slack_conversations_history`, `slack_get_full_conversation`, `slack_get_thread`, `slack_search_messages`, `slack_conversations_unreads`, `slack_users_info`, `slack_list_users`, `slack_users_search` |
| Write | `slack_send_message`, `slack_update_message`, `slack_update_message_unsafe`, `slack_delete_message`, `slack_delete_message_unsafe`, `slack_add_reaction`, `slack_remove_reaction`, `slack_conversations_mark` |
| Workflow profiles | `slack_workflow_save`, `slack_workflows` |
| Hosted only | `slack_smart_search`, `slack_catch_me_up`, `slack_triage` |

The three hosted-only tools come from upstream. Locally they call nothing and
return error `tool_requires_hosted` with upstream's signup links.

### Edit and Delete

Tools `slack_update_message` and `slack_delete_message` act only on your own
messages. Before each write, the tool checks that:

- the signed-in user wrote the message;
- argument `expected_text` matches the message's current text;
- the message is no older than 7 days.

Environment variable `SLACK_MCP_EDIT_DELETE_MAX_AGE_HOURS` changes the window
from its default of 168 hours. Tools `slack_update_message_unsafe` and
`slack_delete_message_unsafe` skip only the age check, for cleanup of older
messages that you ask for by name. Keep both off your client's auto-allow list
so that you confirm each call.

The stdio server lists the `_unsafe` tools. The HTTP server
(`bun src/cli.js http`) hides them unless environment variable
`SLACK_MCP_ENABLE_UNSAFE_TOOLS` is `true`, because HTTP clients may not prompt
before a call.

Each edit or delete attempt writes one `slack-mcp write` JSON line to stderr
with the tool, channel, timestamp, status, error code, and message age. The
line never holds tokens, cookies, or message text.
[Write Log](docs/API.md#write-log) lists the fields.

## Authenticate

The server uses your browser session tokens: an `xoxc-` token and the `xoxd-`
value of cookie `d`. Run the setup helper, which stores them outside the
source tree:

```sh
bunx @uniquedivine/mcp-slack@4.5.2 --setup
```

Environment variables `SLACK_TOKEN` and `SLACK_COOKIE` take precedence over
file `~/.slack-mcp-tokens.json`. Do not commit tokens, cookies, browser
profiles, or local exports.

Slack rotates session tokens every one to two weeks. On macOS, the server
reads fresh tokens from Chrome in three cases:

- A Slack call fails with `invalid_auth` or `token_expired`. The server
  extracts new tokens and retries the call once.
- A health check, which runs every 4 hours, finds that the tokens in file
  `~/.slack-mcp-tokens.json` are more than 10 days old. The server tries at
  most once an hour. Tokens from environment variables have no recorded age,
  so this check never refreshes them.
- You call tool `slack_refresh_tokens` or run `--refresh-tokens`.

Each case needs Chrome running with a signed-in Slack tab at `app.slack.com`.

Chrome auto-refresh runs only on macOS. On Linux, Windows, and WSL, refresh by
hand when a tool returns `invalid_auth` or `slack_auth_expired`:

1. Sign in to Slack at <https://app.slack.com> in a browser.
2. Run `bunx @uniquedivine/mcp-slack@4.5.2 --setup` and paste the token and
   cookie when it asks. [Manual Extraction](docs/SETUP.md#manual-extraction)
   shows where to find them.
3. If you set `SLACK_TOKEN` and `SLACK_COOKIE`, update both or unset both.
   Setup alone has no effect while they are set.
4. Reconnect the MCP server.

## Install From Source

Requirements: Git and Bun 1.4.3 or newer. Linked executables, package scripts,
tests, and CI use Bun directly; no Node installation is needed.

```sh
git clone https://github.com/Unique-Divine/jiyuu.git
cd jiyuu
just --justfile mcp/slack/justfile install
```

Without Just, run `bun install --frozen-lockfile` inside `mcp/slack`. Command
`just link` runs `bun link`, which points command `slack-mcp` and the other
package executables at this checkout. Then configure the client:

```json
{
  "mcpServers": {
    "slack": { "command": "slack-mcp" }
  }
}
```

For a direct source launch, run `bun` with the absolute path to `src/cli.js`.
Restart the MCP connection after you relink or change code. Check the
connected Slack identity with `slack_health_check` before you write.

## Develop

```sh
just install
just test
just check
just run
```

Run these commands from `mcp/slack`. The `bun:test` suite covers tool and
handler parity, rich message fields, edit and delete guards, the write log,
and authentication errors without sending Slack messages. Recipe `check`
builds the entrypoints to an ignored cache to catch import and syntax errors.

HTTP and web modes run through `bun src/cli.js http` and
`bun src/cli.js web`. [docs/WEB-API.md](docs/WEB-API.md) covers the web mode.
The inherited Dockerfile still targets upstream's Node image and is not a
supported install path.

Report issues in [Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/issues)
with `mcp/slack` in the title.

## Attribution and License

This package is a fork of
[jtalk22/slack-mcp-server](https://github.com/jtalk22/slack-mcp-server),
imported through
[Unique-Divine/slack-mcp-server](https://github.com/Unique-Divine/slack-mcp-server).
Its version continues from upstream `@jtalk22/slack-mcp` 4.4.3.
[IMPORT.json](IMPORT.json) and package metadata `jiyuu.upstream` in file
`package.json` pin the source commit. The original
[MIT license and copyright notice](LICENSE) are retained.

Upstream code for its hosted service is still in the tree. When session tokens
expire, the error links to that service. Set `SLACK_MCP_NO_UPSELL=1` to hide
the link. Unique Divine does not run that service.
