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
point it at another network, set both the Keeper and LCD endpoints as
[cli.md](cli.md#switch-networks) shows, then reconnect the client. Never read or edit an MCP client's config file to learn
about the server. Call `mcp_info`.

## Tools

Call `mcp_info` first. It reports the version, endpoints, the active account,
and `trading`, which says whether trades can submit. `restartNeeded` is true
when sai was updated and this server still runs the old code; ask the person
to restart the client. `logs.dir` is where sai logs every tool call, and `sai
logs tail --surface mcp` in a terminal reads them ([cli.md](cli.md#logs)).

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

Account reads take an optional `addr`. Without one they read the active
account and report `addrSource` as `session`. Say so to the person, and pass
`addr` to read any other account.

## Sign-In

The server lists every tool whether or not anyone is signed in. Each
`trade_*` call checks the session of the account it acts as.

| `mcp_info.trading.reason` | A `trade_*` call returns | Meaning |
| --- | --- | --- |
| `signed-in` | the trade result | Trades can submit. |
| `signed-out` | `SIGNED_OUT` | No account is connected. |
| `expired` | `SESSION_EXPIRED` | The session ran out. |
| `other-endpoint` | `SESSION_UNAVAILABLE` | The session belongs to another Keeper endpoint, or its file is unreadable. |

A refused call submits nothing. Ask the person to run `sai auth import`, or
`sai auth login NAME` with their Foundry keystore name, in their own
terminal, then retry. The server never asks
for the token and never returns it.

## Preview, Confirm, Then Trade

1. Call the matching `preview_*` tool with `addr` set to the account the
   person means. Read the account from `auth_list` once, then pass it on
   every preview and trade. Without `addr` a tool acts on whichever account
   is active at that moment, and another terminal or client can switch it.
2. Show the person the preview, the `addr` it names, and the network from
   `mcp_info`. For a market open, also give the worst-case fill described
   below. Wait for explicit confirmation.
3. Call the `trade_*` tool with the same arguments, the same `addr`, and the
   preview's digest.

Outputs use camelCase and inputs use snake_case:

| Preview output | `trade_*` input | Needed by |
| --- | --- | --- |
| `previewDigest` | `preview_digest` | every `trade_*` tool |
| `referencePrice` | `reference_price` | `trade_open` for a market order |

Sending `previewDigest` or `referencePrice` as an input fails argument
validation. Other inputs are snake_case too, such as `market_id`,
`order_type`, `take_profit`, `cancel_order`, and `allow_concurrent_open`.

Before submitting, the server rebuilds the trade and compares its digest with
the one the person confirmed. The digest covers:

- Every trade: the account and the intent the arguments produce.
- `trade_open` for a market order: also the confirmed `reference_price`.
- `trade_margin`: also the whole margin quote computed from the position. If
  the position changed, the quote changes and the digest no longer matches.
- `trade_partial_close`: the collateral reduction computed from the position.
  It does not cover the position's leverage. If leverage changes after the
  preview and the same reduction is still valid, the digest matches and the
  trade submits. The server also reads the position twice during the live
  call and refuses when collateral or leverage differ between those two
  reads, which catches a change during the call, not one since the preview.
  Preview again right before submitting when the position may have changed.

### Market Opens: Quote Check and Fill Bound

A market `trade_open` makes two different price comparisons:

1. **Quote check, by the server.** It reads the price again at submit and
   refuses with `PRICE_MOVED` when that price differs from the confirmed
   `reference_price` by more than the slippage.
2. **Fill bound, by the contract.** The submitted intent carries the price
   read at submit as its `intentPrice`, and the contract allows the fill to
   differ from that price by the full slippage.

The two stack. A long confirmed at 60,000 with 1% slippage passes the quote
check at 60,600 and can fill up to 61,206. The worst-case fill is the
confirmed price moved by the slippage twice: `reference × (1 + s)²` for a
long, `reference × (1 − s)²` for a short. Tell the person that number before
they confirm.

No setting gives a hard ceiling. To keep a long under a ceiling `C` from
reference `R`, choose a slippage with `R × (1 + s)² ≤ C`, so
`s ≤ sqrt(C / R) − 1`, rounded down. For a ceiling of 60,600 from 60,000,
`s ≤ 0.498%`: 0.49% gives a worst case of 60,589.44, and 0.5% allows
60,601.50. For a short with a floor `F`, use `s ≤ 1 − sqrt(F / R)`.

A limit order's `price` is a trigger, not a cap. The contract applies the
slippage after the trigger check, so its worst fill is `trigger × (1 + s)`
for a long and `trigger × (1 − s)` for a short. A stop order has no advance
fill bound: the slippage applies to the oracle price when the stop triggers,
which can sit well past the stop price after a gap.
[trading-safety.md](trading-safety.md) rule 3 has the full rule.

On `PREVIEW_MISMATCH` or `PRICE_MOVED` nothing was submitted. The result
holds a fresh preview. Show it, get confirmation again, and retry with the
new `preview_digest` and `reference_price`.

To cancel a pending order, pass `cancel_order=true` to `preview_close` and
`trade_close`. Without it the tools close a position.

Call `auth_use` only when the person asks to switch accounts.

[trading-safety.md](trading-safety.md) covers what to do after a trade is
accepted, times out, or returns `OUTCOME_UNKNOWN` or `RECENT_OPEN_PENDING`.
