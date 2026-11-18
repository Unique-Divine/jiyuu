# Sai MCP Server

Command `sai mcp serve` runs a stdio MCP server that gives an agent client
the same reads, previews, and trades as the CLI. An agent client launches it.
A person does not run it by hand.

## Register the Server

Install `sai` on `PATH` first (see [SKILL.md](SKILL.md)), then:

```bash
sai mcp install claude          # or codex, cursor, or several at once
sai mcp install --all           # every client found on this machine
sai --json mcp list             # where Sai is registered
sai mcp uninstall codex
```

- `install` registers the `sai` on `PATH`, normally `~/.local/bin/sai`, which
  keeps its path across updates. Results per client are `added`, `replaced`,
  `unchanged`, `failed`, or `planned` with `--dry-run`.
- With no client and no `--all`, `install` asks on a terminal. Without a
  terminal, as when an agent runs it, it lists detected clients and changes
  nothing. Name the client.
- Code `UNSTABLE_PATH` means `sai` is running from the uv cache, as under
  `uvx`. Run `uv tool install git+https://github.com/SaiDotFun/open-sai`
  first.
- `--scope project` writes the project's shared config (Claude Code
  `.mcp.json`, Cursor `.cursor/mcp.json`) instead of the user's. Codex is
  user only.
- From a checkout, give the server command after `--`:
  `sai mcp install claude -- uv run --directory /path/to/open-sai sai mcp serve`.
- The client must restart or reconnect to load a change. Cursor may also ask
  the person to enable `sai` in its MCP settings.
- In `mcp list`, `current` is true when the entry runs the command `install`
  would register.

The server resolves its endpoints once, at start: flags after `sai mcp
serve`, then `SAI_*` variables, then `~/.sai/config.toml`, then mainnet. To
point it at another network, run `sai config set` ([cli.md](cli.md)) and
reconnect the client. Never read or edit an MCP client's config file to learn
about the server. Call `mcp_info`.

## Tools

Call `mcp_info` first. It reports the version, endpoints, the active account,
and `trading`, which says whether trades can submit.

| Tool | Use |
| --- | --- |
| `mcp_info` | Version, endpoints, active account, and whether trading is available |
| `auth_status`, `auth_list` | The active account, and every connected account |
| `auth_use` | Switch the active account. Also switches it for the CLI. |
| `market_list`, `market_prices` | Markets and current indexed prices |
| `account_positions`, `account_balances`, `account_history` | Reads for one account |
| `intent_local`, `intent_status` | Locally recorded intents, and Keeper's record of them |
| `preview_open`, `preview_close`, `preview_partial_close`, `preview_tpsl`, `preview_leverage`, `preview_margin` | What a trade would do. Submits nothing. |
| `trade_open`, `trade_close`, `trade_partial_close`, `trade_tpsl`, `trade_leverage`, `trade_margin` | Submit the previewed trade |

Account reads take an optional `address`. Without one they read the active
account and report `addressSource` as `session`. Say so to the person, and
pass `address` to read any other account.

## Sign-In

The server lists every tool whether or not anyone is signed in. Each
`trade_*` call checks the session of the account it acts as.

| `mcp_info.trading.reason` | A `trade_*` call returns | Meaning |
| --- | --- | --- |
| `signed-in` | the trade result | Trades can submit. |
| `signed-out` | `SIGNED_OUT` | No account is connected. |
| `expired` | `SESSION_EXPIRED` | The session ran out. |
| `other-endpoint` | `SESSION_UNAVAILABLE` | The session belongs to another Keeper endpoint, or its file is unreadable. |

A refused call submits nothing. Ask the person to run `sai auth login` or
`sai auth import` in their own terminal, then retry. The server never asks
for the token and never returns it.

## Preview, Confirm, Then Trade

1. Call the matching `preview_*` tool.
2. Show the person the preview and the account it names. Wait for explicit
   confirmation.
3. Call the `trade_*` tool with the same arguments plus the preview's
   `previewDigest`. A market `trade_open` also takes the preview's
   `referencePrice`.

Before submitting, the server rebuilds the trade and compares it with the
confirmed preview:

- Every trade: the account and the arguments.
- `trade_margin` and `trade_partial_close`: also the position state the
  preview quoted.
- A market `trade_open`: also the price, which may move at most the slippage
  from the confirmed `referencePrice`.

On `PREVIEW_MISMATCH` or `PRICE_MOVED` nothing was submitted. The result
holds a fresh preview. Show it, get confirmation again, and retry with the
new `previewDigest` and `referencePrice`.

Preview and trade tools act on the active account, or on another connected
account passed as `address`. Call `auth_use` only when the person asks to
switch accounts.

[trading-safety.md](trading-safety.md) covers what to do after a trade is
accepted, times out, or returns `OUTCOME_UNKNOWN` or `RECENT_OPEN_PENDING`.
