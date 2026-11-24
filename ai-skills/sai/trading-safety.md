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
3. **Show the worst-case fill from the preview.** Two checks apply to market opens,
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
     short. A CLI dry run estimates the fill using one application of
     slippage to the current Keeper price or to the supplied `--price`.
     The live CLI uses the price it reads at submit, so this estimate can
     move between dry run and submit.
   - Before asking for confirmation, give the person the reported
     `estimates.worstCaseFill` and explain its status.
   - No order type gives a hard price ceiling. To keep an MCP market long
     under a ceiling `C` from reference `R`, the slippage must satisfy
     `R × (1 + s)² ≤ C`, so `s ≤ sqrt(C / R) − 1`, rounded down. For a
     ceiling of 60,600 from 60,000 that is `s ≤ 0.498%`: 0.49% gives a worst
     case of 60,589.44, while 0.5% already allows 60,601.50. For a short with
     a floor `F`, use `s ≤ 1 − sqrt(F / R)`.
   - A limit order is no ceiling either. Its `--price` is a trigger. The
     contract checks the trigger first and applies the slippage after it, so
     the worst fill is `trigger × (1 + s)` for a long and
     `trigger × (1 − s)` for a short. A long limit at 60,600 with 1% slippage
     can fill up to 61,206. State that number too.
   - Stop orders use the same slippage bound around the stored trigger.
     A long stop at 100 with 1% slippage can fill up to 101. If the oracle
     gaps to 105, the contract rejects a fill outside that bound.
   - Keeper's full and partial close intents carry no slippage limit.
   - Prefer MCP tools `preview_open` and `trade_open` when approval depends
     on a quoted price.
4. **Follow the intent to its outcome.** `accepted` means Keeper admitted
   the intent. Use the CLI command `sai intent wait KEY` or MCP tool
   `intent_wait` with the same account. Add `--wait` to a live CLI trade
   to submit once and follow its key in the same command. A limit or stop
   intent returns `placed` when Keeper confirms placement. Its fill can
   happen later.
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
- Look up a missing key with `sai intent local ADDR` or tool `intent_local`.
  Then run `sai --json intent wait KEY --addr ADDR --timeout 30` or call
  `intent_wait` with `key` and `addr`. A timeout returns `unknown` with
  code `OUTCOME_UNKNOWN`. In the CLI this exits 5. Keep checking the same
  key.
- The receipt's `requestTxHash` identifies the submission. For a
  next-oracle fill, cite `execution.block` for the fill. The receipt
  includes `positionNow` and cumulative `positionFeesUsd` when Keeper has
  indexed them. Use `fill` for execution-time evidence.
- To see what happened on this machine, read the logs: `sai --json logs tail
  -n 50 --event intent` shows the intent's status changes by `intentKey`,
  and `--event keeper.request` shows whether the `submit` request was sent
  and what Keeper answered. [cli.md](cli.md#logs) explains the fields. The
  logs are evidence, not the outcome: Keeper's record from `intent status`
  still decides.
- When Keeper has no record of the key five minutes after submission, the CLI
  can mark the local intent `absent` and allow trading again. A delayed
  admission can still appear.
- An intent marked `not_sent` failed before the request left the machine. It
  does not block.
- Tool `trade_open` refuses with `RECENT_OPEN_PENDING` while a recent open is
  unknown or accepted. Check those intents with `intent_status`. Pass
  `allow_concurrent_open=true` only when the person confirms they want
  another open position.
- `sai doctor` settles unknown intents it can resolve by reading Keeper. It
  never places or cancels a trade.

## CLI Errors and Exit Codes

Use `sai --json COMMAND`. Failures write a JSON object with an `error`
containing `code`, `message`, and `retryable`. Check for `hint` and
`intentKey` as well. If a stream has already emitted rows, its final line
contains the error. In text mode, read stderr.

| Exit | Meaning | What to Do |
| --- | --- | --- |
| 0 | Success | Read the result. For trades, follow the intent to its final outcome. |
| 1 | Other failure | Read the code and message. |
| 2 | Command usage error | Check the command's `--help`. |
| 3 | Unsupported install or environment | Follow the installation or terminal instructions in the error. |
| 4 | Request rejected or never sent | Review the error. A revised trade needs a fresh preview and confirmation. |
| 5 | Outcome unknown | Keep `intentKey` and run `sai --json intent status ADDR --key KEY` against the original Keeper endpoint. Do not resubmit. |
| 6 | Signed out or session expired | Ask the person to sign in in their terminal. |

`retryable: true` means Sai classified the failure as a rejection. Get fresh
confirmation for a revised trade. An intent wait can return its outcome and
receipt beside the `error` object. For MCP errors, read the top-level `code`
and follow the same recovery rules for `NOT_SENT`, `ADMISSION_REJECTED`,
`OUTCOME_UNKNOWN`, `SIGNED_OUT`, and `SESSION_EXPIRED`.

## Digest Confirmation in MCP

A preview for a connected account returns `previewDigest`. If a signed-out
`preview_open` returns `addr: null`, preview again for the connected account
after sign-in, then ask for confirmation. A `trade_*` call must send the
confirmed preview's digest as
input `preview_digest`, and a market `trade_open` must also send the
preview's `referencePrice` as `reference_price`. The server rebuilds the
trade and refuses with `PREVIEW_MISMATCH` or `PRICE_MOVED` when the account,
intent, position quote, or quoted price fails the confirmation check.
A refusal submits nothing. Show the returned `preview` if present. If the
result omits it, call the matching `preview_*` tool and wait for a current
quote before asking for confirmation again.

`PRICE_MOVED` is the quote check from rule 3. Passing it does not cap the
fill at the confirmed price plus the slippage.

The partial-close digest covers the close percentage, starting collateral and
leverage, and the contract inputs used to quote the requested slice. The quote
uses the applicable after-hours leverage cap, collateral/USD oracle price, and
Perp's rounding. A change to those inputs requires a new confirmation.
[mcp.md](mcp.md#preview-confirm-then-trade) lists what each digest covers.

## Reading the Estimates

CLI dry runs and MCP previews return the `estimates` object. In the CLI,
both the indented output and `--json` include these fields.

| Field | Inputs and precision |
| --- | --- |
| `fees.amountUsdc` | Perp's decimal fee rates and collateral oracle. Perp floors each component to six-decimal USDC base units. Opens and closes show undiscounted execution fees. The `excludes` list names omitted costs and discounts. |
| `liquidationPrice.price` | Perp's 90% PnL threshold and two 5% liquidation fee components, floored separately. Opens use the reference or trigger as the entry and deduct the estimated opening fees from collateral. Position changes use the stored entry price and the projected collateral and leverage. The estimate excludes accrued borrowing fees. |
| `worstCaseFill.price` | Keeper's market price, the supplied CLI market reference, or the order trigger. Perp rounds each slippage distance down to 18 decimal places. The `side` field identifies an upper bound for a long or a lower bound for a short. |

The `observedAt` field gives the UTC time the CLI or MCP server read a
price. Oracle publication can predate that read. User-supplied references
and triggers have no observation timestamp.

Show `unavailable` when a required input is missing or quoted collateral or
leverage falls outside market limits. An `unbounded` fill has
no advance price limit. A full close leaves no liquidation price. When the after-hours guard is inactive,
TP/SL changes leave the liquidation price unchanged, and margin and leverage
updates have no fill price. With an active or unverified guard, the preview
reports fees and liquidation as unavailable. Full and partial close previews check Perp's live threshold with borrowing
fees. The execution status `would_liquidate` means Perp would close the
entire position at the observed oracle price. A price beyond the market
group's freshness limit produces `would_be_rejected` for immediate
settlement. Queued closes use a fresh oracle update, so estimates for a
stale tick stay unavailable. Otherwise a partial close
can draw on remaining collateral and exceed the leverage cap. Its
resulting liquidation price stays unavailable until settlement.

MCP binds the existing intent, reference price, and position quote. The
`estimates` object is informational. If its figures affect the person's
decision, refresh the preview and confirm the updated figures.

## CLI Trade Commands

Each command's `--help` lists its parameters. These open previews need no
session and send no intent. Add `--as ADDR` when previewing for a connected
account.

```bash
sai --json trade open --market 0 --side long \
  --type market --amount 2 --leverage 2 --slippage 1 --dry-run
sai --json trade open --market 0 --side long \
  --type limit --amount 2 --leverage 2 --price 60000 --slippage 1 --dry-run
```

For position previews, substitute the connected account and a trade ID
from that account. Keep `--dry-run` while reviewing.

```bash
sai --as 0xYOUR_ADDR --json trade partial-close 42 --percent 50 --dry-run
sai --as 0xYOUR_ADDR --json trade tpsl 42 --take-profit 70000 --dry-run
sai --as 0xYOUR_ADDR --json trade leverage 42 --leverage 5 --dry-run
sai --as 0xYOUR_ADDR --json trade margin 42 --direction add --amount 5 \
  --dry-run
sai --as 0xYOUR_ADDR --json trade close 42 --dry-run
sai --as 0xYOUR_ADDR --json trade close 43 --cancel-order --dry-run
sai --json intent local 0xYOUR_ADDR
sai --json intent wait INTENT_KEY --addr 0xYOUR_ADDR --timeout 30
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
  confirmed the account, the network, and those arguments. For limit
  and stop opens, include the bound from rule 3 in the confirmation.

- **Partial close** takes a percentage and recomputes the amount from the
  position at submit. If the person approved "close 50%, about 50 USDC" and
  the position doubles before submit, the live command closes 100 USDC. Ask
  the person to approve the percentage of whatever the position is at submit,
  and say so in those words. To confirm a quoted slice, use MCP tools
  `preview_partial_close` and `trade_partial_close`. Show the person the
  collateral reduction and quoted notional. If
  `quote.closedNotionalIsEstimate` is true, say the notional is an estimate.
  If the collateral oracle price is unavailable, show the USDC estimate
  from `quote.closedNotionalUsdc`. Sai requires a verifiable price to
  submit the partial close. When the price is available, preview again
  and ask for confirmation. Keeper sends the collateral delta to Perp.
  Perp applies the position state and oracle price at execution, so the
  executed notional can differ from the confirmed quote. The after-hours
  risk guard can also reduce the position before Perp closes the requested
  slice.
- **Margin** takes an amount, `--direction add|remove --amount USDC`, and
  computes the target from the effective leverage Perp will use, including
  any applicable after-hours cap. The dry run's target is a quote: adding
  25 USDC to 100 USDC of collateral at 5x previews
  4x, but if the collateral doubles before submit the same command sends
  about 4.44x. Ask the person to approve the amount, and say the CLI
  computes the target leverage at submit. If they approve a specific target
  leverage, use MCP tools `preview_margin` and `trade_margin`, whose digest
  binds the whole quote, or `trade leverage ID --leverage VALUE`, which
  sends the target they named. If `quote.marginChangeIsEstimate` is true,
  tell the person the after-hours guard can deduct fees before Perp applies
  the target leverage. Perp determines the direction from effective leverage
  at execution.
- **Market open** fills from the price at submit. Give the worst case from
  rule 3 before asking.

In every case, run the dry run again right before submitting if the person
took a while to answer, and ask again when it differs.

What each command checks:

- **Market open.** The CLI refreshes the market price before admission and
  includes the reference price in the accepted response. A dry run can
  show a reference price and still reports that submit must refresh it.
- **Partial close.** It reads the live position from the Perp contract, in
  dry-run mode too, and refuses while Keeper's index lags the contract.
- **Margin.** It quotes from live contract state, in dry-run mode too, and
  submits a target leverage, not an amount cap. The margin that moves can
  differ from the quote if the position changes before execution. If the
  index and the contract disagree, the CLI refuses and submits nothing. Wait
  and retry.
- **Close and cancel.** `trade close ID` closes an open position. To cancel a
  pending limit or stop order, add `--cancel-order`. The CLI checks that the
  order is pending before preview and submit. If it fills after the check,
  Perp can close the filled position at market.

An executed trade stays pending locally until the CLI sees matching indexed
trade history.

## What the Session Can Do

The session token authorizes every trade kind on the funds deposited in the
person's Sai account. It cannot withdraw. Keeper cannot revoke it, and it
lasts as long as Keeper issues it, 30 days in production. Tell the person
this when they ask what connecting an agent allows.
