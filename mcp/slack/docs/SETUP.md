# Setup Guide

This guide covers `@uniquedivine/mcp-slack`, maintained by
[Unique Divine](https://uniquedivine.com). The [README](../README.md) has the
short version.

## Prerequisites

- Bun 1.4.3 or newer.
- A Slack account signed in at `app.slack.com` in a browser.
- On macOS only: Google Chrome, for automatic token extraction and refresh.
  The server also stores tokens in the macOS Keychain. Other platforms use the
  token file.

## 1. Install

```bash
bunx @uniquedivine/mcp-slack@latest --version
```

To work from source instead:

```bash
git clone https://github.com/Unique-Divine/jiyuu.git
cd jiyuu/mcp/slack
bun install --frozen-lockfile
```

## 2. Verify the Install

Run these in an empty directory so that a local checkout can't shadow the
package:

```bash
cd "$(mktemp -d)"
bunx @uniquedivine/mcp-slack@latest --version
bunx @uniquedivine/mcp-slack@latest --help
bunx @uniquedivine/mcp-slack@latest --doctor
```

Flags `--version` and `--help` exit `0`. Flag `--doctor` exits with one code:

| Code | Meaning |
|------|---------|
| `0` | Ready |
| `1` | Missing credentials |
| `2` | Invalid or expired credentials |
| `3` | Connectivity or runtime issue |

Flag `--status` is read-only and never runs Chrome extraction.

## 3. Get Slack Tokens

Run the setup wizard:

```bash
bunx @uniquedivine/mcp-slack@latest --setup
```

On macOS, the wizard reads the tokens from Chrome when Slack is open in a
Chrome tab. On other platforms, it asks you to paste them. From a source
checkout, `bun run tokens:auto` runs the macOS extraction and
`bun run tokens:refresh` prompts for manual values.

### Manual Extraction

1. Open <https://app.slack.com> in a browser and sign in.
2. Open DevTools with F12.
3. Under **Application** > **Cookies** > `https://app.slack.com`, copy the
   value of cookie `d`. It starts with `xoxd-`.
4. In the **Console** tab, run:

   ```javascript
   JSON.parse(localStorage.localConfig_v2).teams[Object.keys(JSON.parse(localStorage.localConfig_v2).teams)[0]].token
   ```

5. Copy the result. It starts with `xoxc-`.
6. Paste both values into `--setup`, or set environment variables
   `SLACK_TOKEN` and `SLACK_COOKIE`.

Slack rotates these tokens every one to two weeks. Off macOS, repeat these
steps when a tool returns `invalid_auth` or `slack_auth_expired`.

## 4. Configure a Client

Claude Code:

```bash
claude mcp add slack -- bunx @uniquedivine/mcp-slack@latest
```

Claude Desktop reads file
`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS and
`%APPDATA%\Claude\claude_desktop_config.json` on Windows:

```json
{
  "mcpServers": {
    "slack": {
      "command": "bunx",
      "args": ["@uniquedivine/mcp-slack@latest"]
    }
  }
}
```

Quit Claude Desktop fully and reopen it. On macOS, its log is file
`~/Library/Logs/Claude/mcp-server-slack.log`. The [README](../README.md#install)
lists the Codex and Cursor entries.

## 5. Verify the Connection

Call tool `slack_health_check`. It returns your user and team names.

## Keep Tokens Fresh While the Client Is Closed (macOS)

While it runs on macOS, the server checks token health every 4 hours. It reads
new tokens from Chrome only when the token file is more than 10 days old, or
when a read or send fails with `invalid_auth` or `token_expired`; edit and
delete tools don't retry. Neither path helps when `SLACK_TOKEN` and
`SLACK_COOKIE` are set, because those win over the token file. If no client
runs the server for a week or two, the tokens can expire before either case
happens. A LaunchAgent can run the refresh twice a day
whenever Chrome is open with a Slack tab.

Create file `~/Library/LaunchAgents/com.yourname.slack-token-refresh.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.yourname.slack-token-refresh</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>-c</string>
        <string>exec "$HOME/.bun/bin/bunx" @uniquedivine/mcp-slack@latest --refresh-tokens</string>
    </array>
    <key>StartCalendarInterval</key>
    <array>
        <dict><key>Hour</key><integer>6</integer><key>Minute</key><integer>17</integer></dict>
        <dict><key>Hour</key><integer>18</integer><key>Minute</key><integer>17</integer></dict>
    </array>
    <key>RunAtLoad</key><true/>
    <key>StandardErrorPath</key><string>/tmp/slack-token-refresh.log</string>
</dict>
</plist>
```

Load it and check the log:

```bash
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.yourname.slack-token-refresh.plist
tail /tmp/slack-token-refresh.log
```

LaunchAgents start without your shell profile, so the plist calls `bunx` by
its absolute path. With a Homebrew Bun install, use `/opt/homebrew/bin/bunx`.
Tag `@latest` keeps the refresh job on the same release as the server. If
Chrome isn't running, the log shows "Failed to extract" and the tokens stay as
they were.

For problems, see [TROUBLESHOOTING.md](TROUBLESHOOTING.md).
