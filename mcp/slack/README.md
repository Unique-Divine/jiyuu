# Slack MCP

Read Slack channels, threads and search results, and perform explicitly
authorized writes through your user session. This server is maintained as
part of Jiyuu. It preserves the imported stdio, HTTP and web entrypoints.

## Install

Requirements: [Bun](https://bun.sh) 1.4.3 or newer and a Slack account signed
in through a browser. Store credentials once, then run the published package
with an exact version pin:

```sh
bunx @uniquedivine/mcp-slack@4.4.3 --setup
bunx @uniquedivine/mcp-slack@4.4.3
```

`--setup` writes `~/.slack-mcp-tokens.json`; see [Authenticate](#authenticate)
for the `SLACK_TOKEN` and `SLACK_COOKIE` alternative. CI in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu) builds and
publishes `@uniquedivine/mcp-slack` to npm from this directory when its version
changes on `main`. The pin decides which reviewed version runs.

### Claude Code

```sh
claude mcp add slack -- bunx @uniquedivine/mcp-slack@4.4.3
```

### Codex

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.slack]
command = "bunx"
args = ["@uniquedivine/mcp-slack@4.4.3"]
```

### Cursor

Add to `~/.cursor/mcp.json`, or `.cursor/mcp.json` in a project:

```json
{
  "mcpServers": {
    "slack": {
      "command": "bunx",
      "args": ["@uniquedivine/mcp-slack@4.4.3"]
    }
  }
}
```

### Update

1. Find the newest version with `npm view @uniquedivine/mcp-slack version`.
2. Change the `@4.4.3` pin in the client config to that version.
3. Restart the MCP server in the client (for example `/mcp` in Claude Code)
   or restart the client. `bunx` downloads the new version on the next launch.

The token store and workflow files stay in your home directory, so updating
needs no new credentials.

## Install From Source

Requirements: Git and Bun 1.4.3 or newer. Linked executables, package scripts,
tests and CI use Bun directly; no Node installation or `node` shim is needed.

```sh
git clone https://github.com/Unique-Divine/jiyuu.git
cd jiyuu
just --justfile mcp/slack/justfile install
```

Without Just, run `bun install --frozen-lockfile` inside `mcp/slack`.
To make the existing command names available, run `just link` in that directory.
This uses `bun link` and points them at this source checkout. The executable
shebangs launch Bun directly.

## Authenticate

Use the setup helper, which stores credentials outside the source tree:

```sh
cd mcp/slack
bun src/cli.js --setup
```

The server accepts `SLACK_TOKEN` and `SLACK_COOKIE`, or uses the existing
`~/.slack-mcp-tokens.json` store. Existing local token, workflow and cache
locations are retained. No new credentials are needed for a checkout move.
Do not commit tokens, cookies, browser profiles or local exports.

## Configure an MCP Client From Source

After `just link`, the existing configuration remains valid:

```json
{
  "mcpServers": {
    "slack": { "command": "slack-mcp" }
  }
}
```

For a direct source launch, use `bun` with the absolute path to `src/cli.js`.
Restart the MCP connection after relinking or changing code. Verify the
connected Slack identity before performing writes.

## Develop

```sh
just install
just test
just check
just run
```

Run these commands from `mcp/slack`. The native `bun:test` suite uses `expect`
matchers and covers tool/handler parity, rich
message fields and authentication-error handling without sending Slack messages.
`check` builds the entrypoints to an ignored cache to catch import and syntax
errors. TypeScript conversion and native `bun check` coverage are follow-ups.

The inherited Dockerfile still describes the upstream Node image and is not
the supported Jiyuu installation. Its conversion is tracked separately.

HTTP and web modes remain available through `bun src/cli.js http` and
`bun src/cli.js web`. The inherited hosted-service, pricing and marketing code
has not been removed in this compatibility import. Keeping it is not a
recommendation to deploy or purchase the original hosted service.

Report issues in [Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/issues)
with `mcp/slack` in the title. The version continues from the imported
`@jtalk22/slack-mcp` 4.4.3; `jiyuu.upstream` in `package.json` records that
baseline.

## Origin and License

Imported from `Unique-Divine/slack-mcp-server`, originally forked from
`jtalk22/slack-mcp-server`. [IMPORT.json](IMPORT.json) pins the source commit.
The original [MIT license and copyright notice](LICENSE) are retained.
