# Trading Safety

Read this page before previewing, submitting, or following a trade through
the CLI or the MCP tools. A mistake here spends the person's money.

## Rules

1. **Preview first.** In the CLI, run the trade command with `--dry-run`.
   With MCP tools, call the matching `preview_*` tool. Show the person the
   result and the account it uses, and wait for an explicit yes.
2. **Accepted is not executed.** `accepted` means Keeper admitted the intent.
   The trade is pending until its outcome is observed. Follow it with `sai
   intent status ADDRESS --key KEY` or tool `intent_status`, and report it as
   executed only once that shows it.
3. **Never resubmit blindly.** After a timeout, a transport error, or code
   `OUTCOME_UNKNOWN`, the first request may have gone through. Keep the
   intent key and check it before anything else.
4. **Never handle the token.** Do not ask for a session token, read it from a
   file, or pass it anywhere. The person signs in with `sai auth import` or
   `sai auth login` in their own terminal.
5. **Do not switch accounts by yourself.** Use `sai auth use` or tool
   `auth_use` only when the person asks.
6. **Confirm again when anything changes.** A new price, a new size, or a new
   account needs a new confirmation. An earlier yes does not carry over.

## Unknown Intents

The CLI records each intent's identity locally before it submits. A network
error after submission leaves the outcome unknown.

- An unknown intent blocks new trades on that account until it is resolved.
- Resolve it with `sai --json intent status ADDRESS --key KEY`, or with tools
  `intent_local` then `intent_status`.
- When Keeper has no record of the key five minutes after submission, the CLI
  marks the intent `absent` and trading resumes. Keeper expires an unadmitted
  intent after two minutes, so `absent` means it was not admitted.
- An intent marked `not_sent` failed before the request left the machine. It
  does not block.
- Tool `trade_open` refuses with `RECENT_OPEN_PENDING` while a recent open is
  unknown or accepted. Check those intents with `intent_status`. Pass
  `allow_concurrent_open=true` only when the person confirms they want
  another open position.
- `sai doctor` settles unknown intents it can resolve by reading Keeper. It
  never places or cancels a trade.

## Digest Confirmation in MCP

Each preview returns a `previewDigest`. A `trade_*` call must carry the
digest of the preview the person confirmed, and a market `trade_open` also
its `referencePrice`. The server rebuilds the trade and refuses with
`PREVIEW_MISMATCH` or `PRICE_MOVED` when the account, the arguments, the
quoted position state, or the price no longer match. Nothing is submitted on
a refusal. Show the fresh preview from the result and ask again.
[mcp.md](mcp.md) has the full flow.

## CLI Trade Commands

Each command's `--help` lists its parameters. These examples use `--dry-run`,
a placeholder address, and an existing trade ID. They send nothing.

```bash
sai --json trade open --market 0 --side long --type market \
  --amount 2 --leverage 2 --slippage 1 --dry-run
sai --json trade open --market 0 --side long --type limit \
  --amount 2 --leverage 2 --price 60000 --slippage 1 --dry-run
sai --json trade partial-close 42 --percent 50 --dry-run
sai --json trade tpsl 42 --take-profit 70000 --dry-run
sai --json trade leverage 42 --leverage 5 --dry-run
sai --json trade margin 42 --direction add --amount 5 --dry-run
sai --json trade close 42 --dry-run
sai --json intent local 0xYOUR_ADDRESS
sai --json intent status 0xYOUR_ADDRESS --key INTENT_KEY
```

What each command checks:

- **Market open.** The CLI refreshes the market price before admission and
  includes the reference price in the accepted response. A dry run reports
  that a price refresh is still required.
- **Partial close.** It reads the live position from the Perp contract, in
  dry-run mode too, and refuses while Keeper's index lags the contract.
- **Margin.** It quotes from live contract state, in dry-run mode too, and
  submits a target leverage, not an amount cap. The margin that moves can
  differ from the quote if the position changes before execution. If the
  index and the contract disagree, the CLI refuses and submits nothing. Wait
  and retry.
- **Close.** `trade close` closes a position or cancels a pending order.

An executed trade stays pending locally until the CLI sees matching indexed
trade history.

## What the Session Can Do

The session token authorizes every trade kind on the funds deposited in the
person's Sai account. It cannot withdraw. Keeper cannot revoke it, and it
lasts as long as Keeper issues it, 30 days in production. Tell the person
this when they ask what connecting an agent allows.
