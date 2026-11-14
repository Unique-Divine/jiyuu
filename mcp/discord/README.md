# Discord MCP

Read and manage Discord servers through a bot account over MCP. The server
covers messages, channels, forums, reactions, webhooks, roles and members, with
stdio and streamable HTTP transports. It is maintained as part of Jiyuu.

Imported from [barryyip0625/mcp-discord](https://github.com/barryyip0625/mcp-discord)
at v1.4.0 and run from TypeScript source with Bun. Original source and license:
[IMPORT.json](IMPORT.json), [NOTICE](NOTICE) and [LICENSE](LICENSE).

## Install

Requirements: Git and Bun 1.4.3 or newer. No Node installation or build step
is needed; Bun runs the TypeScript entrypoints directly.

```sh
git clone https://github.com/Unique-Divine/jiyuu.git
cd jiyuu
just --justfile mcp/discord/justfile install
```

Without Just, run `bun install --frozen-lockfile` inside `mcp/discord`. To make
the `mcp-discord` command available, run `just link` in that directory. This
uses `bun link` and points the command at this source checkout.

## Discord Bot Setup

- A Discord bot with appropriate permissions
  - Bot token (obtainable from the [Discord Developer Portal](https://discord.com/developers/applications))
  - Message Content Intent enabled
  - Server Members Intent enabled
  - Presence Intent enabled
- Permissions required in your Discord server:

  #### Easiest Setup
  - Administrator (Recommended for quick setup and full functionality)

  #### Or, select only the required permissions:
  - Send Messages
  - Create Public Threads
  - Send Messages in Threads
  - Manage Messages
  - Manage Threads
  - Manage Channels
  - Manage Webhooks
  - Manage Roles
  - Add Reactions
  - View Channel

- Add your Discord bot to your server
  - To add your Discord bot to your server, use one of the following invite links (replace `INSERT_CLIENT_ID_HERE` with your bot's client ID):
    - **Administrator (full access):**
        https://discord.com/oauth2/authorize?client_id=INSERT_CLIENT_ID_HERE&scope=bot&permissions=8
    - **Custom permissions (minimum required):**
        https://discord.com/oauth2/authorize?client_id=INSERT_CLIENT_ID_HERE&scope=bot&permissions=52076489808

> **Note:**  
> According to Discord's security model, a bot can only access information from servers it has been explicitly added to.  
> If you want to use this MCP server to access a specific Discord server, you must add the bot to that server first.  
> Use the invite link below to add the bot to your target server.

## Configure

The server reads the bot token from `DISCORD_TOKEN`, from a `.env` file in the
working directory, or from `--config <token>`. Prefer the environment: command
line arguments are visible to other local processes. Do not commit tokens or
`.env` files. Without a token the server starts, reports that it skipped
auto-login, and `discord_login` can supply one later.

### Stdio Transport

`src/index.ts` is the stdio entrypoint for command-based MCP clients:

```json
{
  "mcpServers": {
    "discord": {
      "command": "bun",
      "args": ["/path/to/jiyuu/mcp/discord/src/index.ts"],
      "env": { "DISCORD_TOKEN": "your_discord_bot_token" }
    }
  }
}
```

After `just link`, `"command": "mcp-discord"` with no arguments also works.
Unique-Divine/dotfiles manages this link itself starting with
[dotfiles PR #137](https://github.com/Unique-Divine/dotfiles/pull/137): its
`just bun-sync` links `mcp-discord` to this checkout's `src/index.ts`. On a
workstation with that change, don't also run `just link`.
`just run` starts the same entrypoint from this directory.

### Streamable HTTP Transport

`src/app.ts` serves MCP over HTTP at `/mcp`. It does not speak stdio.

```sh
DISCORD_TOKEN=your_discord_bot_token bun src/app.ts --transport http --port 3000
```

The endpoint is then `http://127.0.0.1:3000/mcp`. `src/index.ts --transport http`
also serves HTTP, on port 8080 unless `--port` is given.

The HTTP transports have no authentication. Anyone who can reach the port can
call every tool with the bot's permissions, including sends and deletes. Both
entrypoints bind to `127.0.0.1` by default; keep them on loopback. `--host` or
`HTTP_HOST` overrides the address, and the server logs a warning for any
non-loopback host. Upstream bound to all interfaces.

Both entrypoints also reject requests whose `Host` header is not
`127.0.0.1`, `localhost` or `[::1]` on the bound port, or the explicit
`--host`, with HTTP 403. A browser `Origin`, when sent, must match one of those
too. This blocks DNS rebinding, where a web page points its own hostname at
127.0.0.1. Requests without `Origin`, as MCP clients send them, are allowed.
`HTTP_ALLOWED_HOSTS` (comma-separated `host[:port]`) adds names, which a
wildcard bind such as `0.0.0.0` needs for remote clients.

## Tools Documentation

### Basic Functions

- `discord_login`: Login to Discord using the configured token
- `discord_list_servers`: List all Discord servers the bot is a member of
- `discord_send`: Send a message to a specified channel
- `discord_get_server_info`: Get Discord server information

### Channel Management

- `discord_create_text_channel`: Create a text channel
- `discord_create_forum_channel`: Create a forum channel
- `discord_create_voice_channel`: Create a voice channel
- `discord_edit_channel`: Edit a channel name, topic, parent category, or position
- `discord_delete_channel`: Delete a channel
- `discord_create_category`: Create a channel category
- `discord_edit_category`: Edit a channel category
- `discord_delete_category`: Delete a channel category
- `discord_set_channel_permissions`: Set permission overrides for a role or user on a channel/category
- `discord_remove_channel_permissions`: Remove permission overrides from a channel/category

### Forum Functions

- `discord_get_forum_channels`: Get a list of forum channels
- `discord_create_forum_post`: Create a forum post
- `discord_get_forum_post`: Get a forum post
- `discord_list_forum_threads`: List active and archived threads in a forum channel
- `discord_reply_to_forum`: Reply to a forum post
- `discord_get_forum_tags`: Get all available tags for a forum channel
- `discord_set_forum_tags`: Replace the available tags for a forum channel
- `discord_update_forum_post`: Update a forum post's title, tags, archived state, or locked state
- `discord_delete_forum_post`: Delete a forum post

### Messages and Reactions

- `discord_search_messages`: Search messages in a server
- `discord_read_messages`: Read channel messages (supports `before`, `after`, `around` params — accepts snowflake IDs or ISO 8601 dates like `"2025-03-01T00:00:00Z"`)
- `discord_edit_message`: Edit a bot-authored message
- `discord_add_reaction`: Add a reaction to a message
- `discord_add_multiple_reactions`: Add multiple reactions to a message
- `discord_remove_reaction`: Remove a reaction from a message
- `discord_delete_message`: Delete a specific message from a channel

### Webhook Management

- `discord_create_webhook`: Creates a new webhook for a Discord channel
- `discord_send_webhook_message`: Sends a message to a Discord channel using a webhook
- `discord_edit_webhook`: Edits an existing webhook for a Discord channel
- `discord_delete_webhook`: Deletes an existing webhook for a Discord channel

### Role Management

- `discord_list_roles`: List all roles in a server
- `discord_create_role`: Create a role with color, hoist, mentionable, and permission options
- `discord_edit_role`: Edit an existing role
- `discord_delete_role`: Delete a role
- `discord_assign_role`: Assign a role to a member
- `discord_remove_role`: Remove a role from a member

### Member Management

- `discord_list_members`: List members in a server with roles
- `discord_get_member`: Get detailed information about a specific member

## Development

```sh
just test   # bun:test suite, each file isolated so module mocks do not leak
just check  # bun check, then a Bun build of both entrypoints
```

## License

MIT. See [LICENSE](LICENSE) for the original copyright notice and
[NOTICE](NOTICE) for provenance.
