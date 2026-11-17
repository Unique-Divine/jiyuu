# MCP Servers

Discord, Slack and Telegram MCP servers maintained in Jiyuu. Slack and Telegram
use your existing user accounts; Discord uses a bot token. Sending messages and
other writes require explicit authorization.

| Directory | Package | Pinned install | Runtime |
| --- | --- | --- | --- |
| [discord](discord/README.md) | npm `@uniquedivine/mcp-discord` | `bunx @uniquedivine/mcp-discord@1.3.8` | Bun 1.4.3+ |
| [slack](slack/README.md) | npm `@uniquedivine/mcp-slack` | `bunx @uniquedivine/mcp-slack@4.4.3` | Bun 1.4.3+ |
| [telegram](telegram/README.md) | PyPI `uniquedivine-telegram-mcp` | `uvx uniquedivine-telegram-mcp@2.0.1` | Python 3.10+ and uv |

Each component README has the client configuration for Claude Code, Codex and
Cursor, and the update steps. CI publishes a package from `main` when its
version changes. Each package has its own plain semver version; the upstream
baseline it started from is recorded as `jiyuu.upstream` in `package.json` or
`[tool.jiyuu.upstream]` in `pyproject.toml`.

From a source checkout, each directory has `just install`, `just test`,
`just check` and `just run`.

Run the `just` commands inside the component directory. From the repository
root, `just mcp-install`, `just mcp-test` and `just mcp-check` operate on all
three.

Report issues in [Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/issues)
with `mcp/discord`, `mcp/slack` or `mcp/telegram` in the title. The packages
are published under Jiyuu's own names; the upstream packages are separate.

## Source Baselines

Each component's `IMPORT.json` records the source snapshot and exclusions. Git
history was not imported. Original licenses and applicable notices remain with
the components; Jiyuu's root license does not replace them.

See [upstreams.json](upstreams.json) for original upstreams, the maintained Slack
fork and immutable comparison baselines. Compare against these commits rather
than treating the import date as a completed upstream review. Discord is pinned
at upstream v1.4.0, the revision the dotfiles build today; upgrading to v1.5.2
needs its own review.

The first import preserves existing functionality, including Slack's optional
web and hosted-service code. Bun is the explicit Slack runtime, including executable shebangs and native
`bun:test` tests. Removing commercial features and migrating Slack to TypeScript
remain separate work.
