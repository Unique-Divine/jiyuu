# MCP Servers

Discord, Slack and Telegram MCP servers maintained in Jiyuu. Slack and Telegram
use your existing user accounts; Discord uses a bot token. Sending messages and
other writes require explicit authorization.

| Directory | Package | Pinned install | Runtime |
| --- | --- | --- | --- |
| [discord](discord/README.md) | npm `@uniquedivine/mcp-discord` | `bunx @uniquedivine/mcp-discord@1.3.9` | Bun 1.4.3+ |
| [slack](slack/README.md) | npm `@uniquedivine/mcp-slack` | `bunx @uniquedivine/mcp-slack@4.5.3` | Bun 1.4.3+ |
| [telegram](telegram/README.md) | PyPI `uniquedivine-telegram-mcp` | `uvx uniquedivine-telegram-mcp@2.0.3` | Python 3.10+ and uv |

Each component README has client configuration for Claude Code, Codex, and
Cursor, plus update steps. CI publishes a package from branch `main` when its
version changes. Each package has its own plain semver version. Package
metadata `jiyuu.upstream` in file `package.json`, or table
`[tool.jiyuu.upstream]` in file `pyproject.toml`, records the upstream baseline.

## Version and Update Notices

Each server has a read-only `mcp_info` tool. It reports the running version,
whether the server runs from a source checkout or an installed package, the
git commit for a checkout, the upstream baseline, and update status. The
update fields are `latestVersion`, `updateAvailable`, `checkedAt`,
`pinCommand`, `source`, and `notChecked`. The report lists environment
variable names, never values. Agents should call `mcp_info` instead of
reading MCP client configs, which hold secrets.

Two callers read the latest published version from npm or PyPI, each with a
2-second timeout. Both share a cache under `$XDG_CACHE_HOME/jiyuu-mcp/`
(default `~/.cache/jiyuu-mcp/`).

| | Startup notice | Tool `mcp_info` |
| --- | --- | --- |
| Reads the registry | When the cache is older than 1 hour | On every call |
| Cache | Read and written | Written after a registry read; read only when the registry can't be reached |
| Output | One line on the first successful tool result, only when a newer version exists | The answer with `checkedAt` and `source` |

Field `source` is `live` when the call read the registry and `cache` when it
fell back to the cache. Neither caller trusts a cached latest version older
than the running version, and `pinCommand` is set only when the latest
version is newer than the running one, so the report never suggests a
downgrade. The startup notice fails silently. Its line looks like this:

```text
Update available: uniquedivine-telegram-mcp 2.1.0 (running 2.0.1). Change the pin to `uvx uniquedivine-telegram-mcp@2.1.0` and reload the MCP server.
```

The servers never update themselves; the pin in the client config decides
what runs. Set `JIYUU_MCP_NO_UPDATE_CHECK=1` to turn the check off.

The TypeScript module is copied, byte for byte, to
`discord/src/jiyuuMcpInfo.ts` and `slack/lib/jiyuuMcpInfo.ts`, so each
published package runs alone. Tests in both packages fail if the copies
differ. Telegram's equivalent lives in `telegram/telegram_mcp/update_check.py`
and `telegram/telegram_mcp/server_info.py`.

## Development

From a source checkout, each directory has recipes `just install`,
`just test`, `just check`, and `just run`.

Run the `just` commands inside the component directory. From the repository
root, `just mcp-install`, `just mcp-test` and `just mcp-check` operate on all
three.

Report issues in [Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/issues)
with `mcp/discord`, `mcp/slack` or `mcp/telegram` in the title. Jiyuu
publishes these packages under its own names. The upstream packages are
separate projects.

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
