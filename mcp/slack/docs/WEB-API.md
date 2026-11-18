# Web API Reference

The web server (`bun src/cli.js web`) serves a local dashboard and REST
endpoints for reading Slack, sending messages, marking conversations read,
and adding or removing reactions. It doesn't expose the edit and delete tools.
For MCP over HTTP, use `bun src/cli.js http` instead.

## Start the Server

```bash
bunx @uniquedivine/mcp-slack@latest web
# From a source checkout:
bun run web
```

The server listens on `127.0.0.1:3000`, serves the dashboard at
<http://localhost:3000>, and writes an API key to file `~/.slack-mcp-api-key`
on first run.

## Auto-Start on Login

On macOS, a LaunchAgent starts the web server at login:

```bash
cat > ~/Library/LaunchAgents/com.slack-web-api.plist << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.slack-web-api</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/YOUR_USERNAME/.bun/bin/bunx</string>
        <string>@uniquedivine/mcp-slack@latest</string>
        <string>web</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/slack-web-api.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/slack-web-api.error.log</string>
</dict>
</plist>
EOF

# Load it
launchctl load ~/Library/LaunchAgents/com.slack-web-api.plist
```

**Manage the service:**
```bash
# Check status
launchctl list | grep slack-web-api

# Restart
launchctl kickstart -k gui/$(id -u)/com.slack-web-api

# Stop
launchctl unload ~/Library/LaunchAgents/com.slack-web-api.plist
```

## Authentication

All API requests require the API key in the Authorization header:

```
Authorization: Bearer <your-api-key>
```

The server writes a random API key to file `~/.slack-mcp-api-key` on first run.
The startup log shows only its first 8 characters. If `SLACK_API_KEY` is set,
the server uses it and ignores the file. This command picks the same key:

```bash
API_KEY="${SLACK_API_KEY:-$(cat ~/.slack-mcp-api-key)}"
```

To use a custom key:
```bash
SLACK_API_KEY=your-custom-key bunx @uniquedivine/mcp-slack@latest web
```

---

## Endpoints

### GET /
Server info and available endpoints (no auth required).

### GET /health
Check Slack connection status.

```bash
curl -H "Authorization: Bearer $API_KEY" http://localhost:3000/health
```

**Response:**
```json
{
  "status": "ok",
  "code": "ok",
  "message": "Slack auth valid",
  "user": "unique",
  "team": "Example Workspace"
}
```

---

### GET /token-status
Read-only token diagnostics (age/health/cache stats). This endpoint does not trigger Chrome extraction.

```bash
curl -H "Authorization: Bearer $API_KEY" http://localhost:3000/token-status
```

---

### POST /refresh
Force token refresh from Chrome.

```bash
curl -X POST -H "Authorization: Bearer $API_KEY" http://localhost:3000/refresh
```

---

### GET /conversations
List conversations (DMs/channels).

**Query Parameters:**
| Param | Default | Description |
|-------|---------|-------------|
| types | im,mpim | Conversation types |
| limit | 100 | Max results |

**Types:** `im`, `mpim`, `public_channel`, `private_channel`

```bash
curl -H "Authorization: Bearer $API_KEY" "http://localhost:3000/conversations?types=im"
```

**Response:**
```json
{
  "count": 5,
  "conversations": [
    { "id": "D063M4403MW", "name": "Gwen Santos", "type": "dm" }
  ]
}
```

---

### GET /conversations/:id/history
Get messages from a conversation.

**Query Parameters:**
| Param | Default | Description |
|-------|---------|-------------|
| limit | 50 | Messages to fetch |
| oldest | - | Unix timestamp start |
| latest | - | Unix timestamp end |
| resolve_users | true | Convert user IDs to names |

```bash
curl -H "Authorization: Bearer $API_KEY" "http://localhost:3000/conversations/D063M4403MW/history?limit=10"
```

---

### GET /conversations/:id/full
Export full conversation with threads.

**Query Parameters:**
| Param | Default | Description |
|-------|---------|-------------|
| max_messages | 2000 | Max messages |
| include_threads | true | Fetch replies |
| oldest | - | Unix timestamp start |
| latest | - | Unix timestamp end |
| output_file | - | Save to file path |

```bash
curl -H "Authorization: Bearer $API_KEY" "http://localhost:3000/conversations/D063M4403MW/full?max_messages=500"
```

---

### GET /conversations/:id/thread/:ts
Get thread replies.

```bash
curl -H "Authorization: Bearer $API_KEY" "http://localhost:3000/conversations/D063M4403MW/thread/1767368030.607599"
```

---

### GET /conversations/unreads
List channels and DMs with unread messages, most unread first.

| Param | Default | Description |
|-------|---------|-------------|
| types | im,mpim,public_channel,private_channel | Conversation types |
| limit | 50 | Max conversations |

---

### POST /conversations/:id/mark
Mark a conversation read up to a timestamp. Body: `{"timestamp": "1767368030.607599"}`.

---

### GET /search
Search messages across workspace.

**Query Parameters:**
| Param | Default | Description |
|-------|---------|-------------|
| q | *required* | Search query |
| count | 20 | Max results |

**Query Syntax:**
- `from:@username`: From a user.
- `in:#channel`: In a channel.
- `before:2026-01-01`: Before a date.
- `after:2025-12-01`: After a date.

```bash
curl -H "Authorization: Bearer $API_KEY" "http://localhost:3000/search?q=from:@gwen%20meeting"
```

---

### POST /messages
Send a message.

**Body:**
```json
{
  "channel_id": "D063M4403MW",
  "text": "Hello!",
  "thread_ts": "1767368030.607599"  // optional, for replies
}
```

```bash
curl -X POST -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"channel_id":"D063M4403MW","text":"Hello!"}' \
  http://localhost:3000/messages
```

---

### GET /users
List all workspace users.

```bash
curl -H "Authorization: Bearer $API_KEY" "http://localhost:3000/users?limit=50"
```

---

### GET /users/search
Search users by name, display name, or email. Parameters: `q` (required) and
`limit` (default 20).

---

### GET /users/:id
Get user details.

```bash
curl -H "Authorization: Bearer $API_KEY" http://localhost:3000/users/U05GPEVH7J9
```

---

### POST /reactions and DELETE /reactions
Add or remove a reaction. Body: `channel_id`, `timestamp`, and `reaction`, the
emoji name without colons.

```bash
curl -X POST -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"channel_id":"D063M4403MW","timestamp":"1767368030.607599","reaction":"thumbsup"}' \
  http://localhost:3000/reactions
```

---

## Web UI

Open <http://localhost:3000> for the dashboard. On first load it asks for the
API key: paste the contents of `~/.slack-mcp-api-key`, or your `SLACK_API_KEY`
value. Check **Remember this key on this device** to keep the key in browser storage; otherwise the
dashboard forgets it when the tab closes. You can also open
`http://localhost:3000/?key=<api-key>` once; the dashboard stores the key for
that tab and removes it from the URL.

The dashboard lets you pick a conversation, read messages and threads,
search, and send messages.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | 3000 | Server port |
| `SLACK_API_KEY` | Generated | API key for requests |
| `SLACK_TOKEN` | None | Slack `xoxc-` token |
| `SLACK_COOKIE` | None | Slack `xoxd-` cookie |

## Security Notes

- Anyone with the API key can read and post as you. Keep it secret.
- The server binds to `127.0.0.1`, and CORS allows only localhost origins.
- For remote access, put it behind a reverse proxy with HTTPS.
