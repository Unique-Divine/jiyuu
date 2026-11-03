# Slack MCP

Read Slack channels, threads and search results, and perform explicitly
authorized writes through your user session. This server is maintained as
part of Jiyuu. It preserves the imported stdio, HTTP and web entrypoints.

## Install

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

## Configure an MCP Client

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
with `mcp/slack` in the title. This checkout is installed from source; inherited
package names and version numbers identify compatibility, not a Jiyuu registry
release. Package publishing is disabled.

## Origin and License

Imported from `Unique-Divine/slack-mcp-server`, originally forked from
`jtalk22/slack-mcp-server`. [IMPORT.json](IMPORT.json) pins the source commit.
The original [MIT license and copyright notice](LICENSE) are retained.
