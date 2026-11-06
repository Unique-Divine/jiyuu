# MCP Servers

Slack and Telegram MCP servers maintained in Jiyuu. Both use your existing user
accounts; sending messages and other writes require explicit authorization.

| Directory | Runtime | Commands |
| --- | --- | --- |
| [slack](slack/README.md) | Bun 1.4.3+ | `just install`, `just test`, `just check`, `just run` |
| [telegram](telegram/README.md) | Python 3.13 and uv | `just install`, `just test`, `just check`, `just run` |

Run the listed commands inside the component directory. From the repository
root, `just mcp-install`, `just mcp-test` and `just mcp-check` operate on both.
MCP clients launch the entrypoints documented in each component README directly.

Report issues in [Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/issues)
with `mcp/slack` or `mcp/telegram` in the title. These are source installations;
this repository does not publish the imported upstream packages.

## Source Baselines

Each component's `IMPORT.json` records the source snapshot and exclusions. Git
history was not imported. Original licenses and applicable notices remain with
the components; Jiyuu's root license does not replace them.

See [upstreams.json](upstreams.json) for original upstreams, the maintained Slack
fork and immutable comparison baselines. Compare against these commits rather
than treating the import date as a completed upstream review.

The first import preserves existing functionality, including Slack's optional
web and hosted-service code. Bun is the explicit Slack runtime, including executable shebangs and native
`bun:test` tests. Removing commercial features and migrating Slack to TypeScript
remain separate work.
