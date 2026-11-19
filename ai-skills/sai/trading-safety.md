# Trading Safety

Read this page before previewing, submitting, or following a trade through
the CLI or the MCP tools. A mistake here spends the person's money.

## Rules

1. **Preview first.** With MCP tools, call the matching `preview_*` tool. In
   the CLI, run the trade command with `--dry-run`. Show the person the
   result and the account it names, and wait for an explicit yes.
2. **Pin the account and the network on both calls.** Each call resolves
   the active account and the Keeper endpoint by itself. Another terminal or
   MCP client can switch the account, and a changed setting can point the
   next command at another network, while the person decides.
   - CLI: put the same `sai --endpoint URL --as ADDR` on the dry run and on
     the live command. Read the URL once with `sai config get
     keeper.endpoint`.
   - MCP: pass the same `addr` to the `preview_*` and `trade_*` tools. The
     server fixes its endpoints when it starts; read them from `mcp_info`,
     and read them again if the client reconnected since the preview.
   - Name the account and the network (mainnet or Testnet2) when asking for
     confirmation, and check that the preview's `addr` is the account the
     person named. Trade IDs are per account and per network: `trade close
     42` elsewhere closes a different position.
3. **State the worst-case fill of a market order.** Two separate checks apply,
   and neither holds the fill to the quoted price plus one slippage.
   - The quote check, MCP only: `trade_open` refuses with `PRICE_MOVED` when
     the price it reads at submit differs from the confirmed
     `reference_price` by more than the slippage.
   - The fill bound, CLI and MCP: the submitted intent carries the price read
     at submit, not the confirmed one, and the contract applies the full
     slippage around that submitted price.
   - Together: a long confirmed at 60,000 with 1% slippage passes the quote
     check at 60,600, and can then fill up to 61,206. Through MCP the worst
     case is the confirmed price moved by the slippage twice,
     `reference × (1 + s)²` for a long and `reference × (1 − s)²` for a
     short. Through the CLI there is no quote check: the dry run shows no
     price, and the worst case is the market price at submit moved by the
     slippage once, with no tie to any price the person saw.
   - Before asking for confirmation, compute the worst-case fill and give
     the person that number, not only the quote.
   - No order type gives a hard price ceiling. To keep an MCP market long
     under a ceiling `C` from reference `R`, the slippage must satisfy
     `R × (1 + s)² ≤ C`, so `s ≤ sqrt(C / R) − 1`, rounded down. For a
     ceiling of 60,600 from 60,000 that is `s ≤ 0.498%`: 0.49% gives a worst
     case of 60,589.44, while 0.5% already allows 60,601.50. For a short with
     a floor `F`, use `s ≤ 1 − sqrt(F / R)`.
   - A limit or stop order is no ceiling either. Its `--price` is a trigger.
     The contract checks the trigger first and applies the slippage after it,
     so the worst fill is `trigger × (1 + s)` for a long and
     `trigger × (1 − s)` for a short. A long limit at 60,600 with 1% slippage
     can fill up to 61,206. State that number too.
   - Prefer MCP tools `preview_open` and `trade_open` when approval depends
     on a quoted price.
4. **Accepted is not executed.** `accepted` means Keeper admitted the intent.
   The trade is pending until its outcome is observed. Follow it with `sai
   intent status ADDR --key KEY` or tool `intent_status`, and report it as
   executed only once that shows it.
5. **Never resubmit blindly.** After a timeout, a transport error, or code
   `OUTCOME_UNKNOWN`, the first request may have gone through. Keep the
   intent key and check it before anything else.
6. **Never handle the token.** Do not ask for a session token, read it from a
   file, or pass it anywhere. The person signs in with `sai auth import` or
   `sai auth login NAME` in their own terminal.
7. **Do not switch accounts by yourself.** Use `sai auth use` or tool
   `auth_use` only when the person asks. Pinning with `--as` or `addr`
   switches nothing.
8. **Confirm again when anything changes.** A new price, a new size, or a new
   account needs a new confirmation. An earlier yes does not carry over.

## Unknown Intents

The CLI records each intent's identity locally before it submits. A network
error after submission leaves the outcome unknown.

- An unknown intent blocks new trades on that account until it is resolved.
- Resolve it with `sai --json intent status ADDR --key KEY`, or with tools
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

Each preview returns `previewDigest`. A `trade_*` call must send it back as
input `preview_digest`, and a market `trade_open` must also send the
preview's `referencePrice` as `reference_price`. The server rebuilds the
trade and refuses with `PREVIEW_MISMATCH` or `PRICE_MOVED` when the account,
the intent, the margin quote, or the quoted price no longer match. Nothing is
submitted on a refusal. Show the fresh preview from the result and ask again.

`PRICE_MOVED` is the quote check from rule 3. Passing it does not cap the
fill at the confirmed price plus the slippage.

The digest of a partial close covers the computed collateral reduction, not
the position's leverage. Preview again right before submitting if the
position may have changed. [mcp.md](mcp.md#preview-confirm-then-trade) lists
what each digest covers.

## CLI Trade Commands

Each command's `--help` lists its parameters. These examples use `--dry-run`,
a placeholder account, and an existing trade ID. They send nothing.

```bash
sai --as 0xYOUR_ADDR --json trade open --market 0 --side long \
  --type market --amount 2 --leverage 2 --slippage 1 --dry-run
sai --as 0xYOUR_ADDR --json trade open --market 0 --side long \
  --type limit --amount 2 --leverage 2 --price 60000 --slippage 1 --dry-run
sai --as 0xYOUR_ADDR --json trade partial-close 42 --percent 50 --dry-run
sai --as 0xYOUR_ADDR --json trade tpsl 42 --take-profit 70000 --dry-run
sai --as 0xYOUR_ADDR --json trade leverage 42 --leverage 5 --dry-run
sai --as 0xYOUR_ADDR --json trade margin 42 --direction add --amount 5 \
  --dry-run
sai --as 0xYOUR_ADDR --json trade close 42 --dry-run
sai --as 0xYOUR_ADDR --json trade close 43 --cancel-order --dry-run
sai --json intent local 0xYOUR_ADDR
sai --json intent status 0xYOUR_ADDR --key INTENT_KEY
```

`--as ADDR` needs `ADDR` to be a connected account (`sai auth list`). Trade
output names the account in field `addr`. Add the same `--endpoint URL` to
both calls, as rule 2 says; the examples leave it out to stay short.

### When It Is Safe to Drop `--dry-run`

The CLI has no digest. The live command builds the trade again from current
state and does not compare it with the dry run the person saw. Rerunning the
command without `--dry-run` submits what the person approved only when the
approval still describes the trade:

- **Limit and stop opens, `tpsl`, `leverage`, `close`, and `--cancel-order`**
  send exactly the arguments shown. Drop `--dry-run` once the person has
  confirmed the account, the network, and those arguments. For a limit or
  stop open, the confirmation covers the worst fill from rule 3,
  `trigger × (1 + s)` for a long, not the trigger alone.
- **Partial close** takes a percentage and recomputes the amount from the
  position at submit. If the person approved "close 50%, about 50 USDC" and
  the position doubles before submit, the live command closes 100 USDC. Ask
  the person to approve the percentage of whatever the position is at submit,
  and say so in those words. If they approve a fixed amount, use MCP tools
  `preview_partial_close` and `trade_partial_close`, whose digest binds the
  computed reduction.
- **Margin** takes an amount, `--direction add|remove --amount USDC`, and
  computes the target leverage from the position at submit. The dry run's
  target is a quote: adding 25 USDC to 100 USDC of collateral at 5x previews
  4x, but if the collateral doubles before submit the same command sends
  about 4.44x. Ask the person to approve the amount, and say the CLI
  computes the target leverage at submit. If they approve a specific target
  leverage, use MCP tools `preview_margin` and `trade_margin`, whose digest
  binds the whole quote, or `trade leverage ID --leverage VALUE`, which
  sends the target they named.
- **Market open** fills from the price at submit. Give the worst case from
  rule 3 before asking.

In every case, run the dry run again right before submitting if the person
took a while to answer, and ask again when it differs.

What each command checks:

- **Market open.** The CLI refreshes the market price before admission and
  includes the reference price in the accepted response. A dry run reports
  that a price refresh is still required and shows no price. See rule 3.
- **Partial close.** It reads the live position from the Perp contract, in
  dry-run mode too, and refuses while Keeper's index lags the contract.
- **Margin.** It quotes from live contract state, in dry-run mode too, and
  submits a target leverage, not an amount cap. The margin that moves can
  differ from the quote if the position changes before execution. If the
  index and the contract disagree, the CLI refuses and submits nothing. Wait
  and retry.
- **Close and cancel.** `trade close ID` closes an open position. To cancel a
  pending limit or stop order, add `--cancel-order`. Without the flag the CLI
  sends a close-position intent for that ID.

An executed trade stays pending locally until the CLI sees matching indexed
trade history.

## What the Session Can Do

The session token authorizes every trade kind on the funds deposited in the
person's Sai account. It cannot withdraw. Keeper cannot revoke it, and it
lasts as long as Keeper issues it, 30 days in production. Tell the person
this when they ask what connecting an agent allows.
