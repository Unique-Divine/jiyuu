# Troubleshooting Guide

Fixes for common problems with `@uniquedivine/mcp-slack`. If none of these
help, open an issue in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/issues) with
`mcp/slack` in the title.

## Install Check

If first-run setup fails, check command resolution in an empty directory:

```bash
cd "$(mktemp -d)"
bunx @uniquedivine/mcp-slack --version
bunx @uniquedivine/mcp-slack --help
bunx @uniquedivine/mcp-slack --doctor
```

Flag `--doctor` exits `0` when ready, `1` for missing credentials, `2` for
invalid or expired credentials, and `3` for a connectivity or runtime issue.
If `--version` fails, the problem is the Bun install or `PATH`, not Slack
credentials.

## Expired Tokens

**Symptom:** Tools return `invalid_auth`, `token_expired`, or
`slack_auth_expired`.

**Cause:** Slack rotates browser session tokens (`xoxc-` and `xoxd-`) every
one to two weeks.

The server looks for tokens in this order:

1. Environment variables `SLACK_TOKEN` and `SLACK_COOKIE`.
2. File `~/.slack-mcp-tokens.json`.
3. The macOS Keychain.
4. Chrome extraction, on macOS only.

**Fix on macOS:** With Slack open in a Chrome tab, call tool
`slack_refresh_tokens`, or run
`bunx @uniquedivine/mcp-slack --refresh-tokens`.

**Fix everywhere else:** Chrome auto-refresh runs only on macOS. Sign in to
Slack in a browser, run `bunx @uniquedivine/mcp-slack --setup`, and paste
fresh values. [Manual Extraction](SETUP.md#manual-extraction) shows where to
find them. Then reconnect the MCP server.

If `SLACK_TOKEN` and `SLACK_COOKIE` are set, they win over the token file.
Update both or unset both; `--setup` alone has no effect while they are set.

## Edit or Delete Refused

**`message_too_old`:** Tools `slack_update_message` and
`slack_delete_message` refuse messages older than 7 days. Set
`SLACK_MCP_EDIT_DELETE_MAX_AGE_HOURS` on the server to change the window, or
ask for `slack_update_message_unsafe` or `slack_delete_message_unsafe`, which
skip only the age check.

**`unknown_tool` for an `_unsafe` tool over HTTP:** The HTTP server hides
the `_unsafe` tools unless `SLACK_MCP_ENABLE_UNSAFE_TOOLS=true`.

**`not_own_message` or `text_mismatch`:** The tools act only on your own
messages, and argument `expected_text` must match the message's current text.
Re-read the message with `slack_conversations_history` or `slack_get_thread`
and pass its text.

[API.md](API.md#slack_delete_message) lists every error code.

## DMs Missing

**Symptom:** `slack_list_conversations` returns channels but no DMs.

Slack's `conversations.list` method doesn't return DMs for `xoxc-` tokens. By
default, `slack_list_conversations` adds DMs only from its cache in file
`~/.slack-mcp-dm-cache.json`, and the server ignores that cache once it is
more than 24 hours old. To rebuild it, call the tool with
`discover_dms: true` and `types: "im,mpim"`. The server then reads the first
200 users from `users.list`, calls `conversations.open` for each one, 50 ms
apart, and saves the DMs it finds.

Discovery is slow and can hit rate limits. It also calls `conversations.open`
for users you have never messaged, which can add empty DMs to your Slack
sidebar.

## Rate Limits

**Symptom:** `ratelimited` errors.

The Slack client in file `lib/slack-client.js` waits for Slack's
`Retry-After` header, or 5 seconds without one, and retries up to 3 times
with a longer wait each time. If you still hit limits, lower `limit` on list
calls.

## HTTP Server Returns 503 or 401

Endpoint `/mcp` on the HTTP server (`bun src/cli.js http`) requires a bearer
token.

- `503 http_auth_token_missing`: set `SLACK_MCP_HTTP_AUTH_TOKEN`.
- `401 unauthorized`: the request has no bearer token or the wrong one.

```bash
curl http://localhost:3000/mcp \
  -H "Authorization: Bearer $SLACK_MCP_HTTP_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0.0"}}}'
```

For local testing only, `SLACK_MCP_HTTP_INSECURE=1` turns the check off.

## Web Server Problems

The web server writes its API key to file `~/.slack-mcp-api-key`, and the
startup log masks all but the first 8 characters. If `SLACK_API_KEY` is set,
the server uses it instead of the file. This check picks the same key:

```bash
API_KEY="${SLACK_API_KEY:-$(cat ~/.slack-mcp-api-key)}"
curl http://localhost:3000/health -H "Authorization: Bearer $API_KEY"
```

To keep it running after the terminal closes on macOS, use the LaunchAgent
in [WEB-API.md](WEB-API.md#auto-start-on-login).

## Tools Missing in Claude Desktop

1. Quit Claude Desktop fully and reopen it.
2. Check the config syntax:
   `python3 -m json.tool ~/Library/Application\ Support/Claude/claude_desktop_config.json`.
3. Read file `~/Library/Logs/Claude/mcp-server-slack.log`.
4. If the log says Bun wasn't found, put the absolute path from
   `which bunx` in `command`.

## Chrome Extraction Fails (macOS)

**Symptom:** `slack_refresh_tokens` can't extract tokens from Chrome.

Check that:

1. Chrome is running with a Slack tab at `app.slack.com`, not the desktop
   app, and you are signed in.
2. **View** > **Developer** > **Allow JavaScript from Apple Events** is on.
   Or set `SLACK_MCP_EXTRACTION_MODE=leveldb` to skip AppleScript.
3. If Chrome uses a non-default profile or location, set
   `SLACK_MCP_CHROME_PROFILE` or `SLACK_MCP_CHROME_USER_DATA_DIR`.

## Why Browser Tokens Instead of a Slack App?

A Slack app needs workspace approval and OAuth scopes, and it can't read your
DMs the way you see them. Session tokens give the server the same access you
have in Slack's web client. The cost is token rotation every one to two
weeks, which you refresh by hand off macOS.
