# Sai SLP Vaults Query Reference

Use this reference when inspecting Sai SLP Vault health, deposit caps, rewards,
or raw SLP Vault accounting state on Nibiru mainnet.

## Mainnet SLP Vault Addresses

| SLP Vault | Address |
| --- | --- |
| SLP-USDC Vault — Group 0 | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault — Group 0 | `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej` |
| SLP-USDC Vault — Group 1 | `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8` |
| SLP-stNIBI Vault — Group 1 | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |

Group 0 is the main crypto market group. Group 1 is the real-estate / Coded
Estate group. USDC and stNIBI both use 6 decimal base units on Sai.

## Setup

Use the `/nibiru-cli-nibid` skill since this behavior relies heavily on using the
CLI properly.

```bash
PERP="nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph"
VAULT_G0_USDC="nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p"
VAULT_G0_STNIBI="nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej"
VAULT_G1_USDC="nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8"
VAULT_G1_STNIBI="nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj"

sai_q() {
  nibid query wasm contract-state smart "$1" "$2"
}

raw_item() {
  contract="$1"
  key="$2"
  hex=$(printf '%s' "$key" | xxd -p -c 256)
  nibid query wasm contract-state raw "$contract" "$hex" |
    jq -r '.data // empty' | base64 -d
  echo
}
```

## SLP Vault State Overview

This section defines the SLP Vault terms and storage values used in health,
deposit-cap, reward, and raw-state investigations. Use smart queries for the
normal operator view, and raw state only when validating exact storage items or
debugging query surfaces.

### Core Terms

| Term | Meaning | Where to query |
| --- | --- | --- |
| Collateral | The asset deposited into an SLP Vault, such as USDC or stNIBI. Amounts are stored in base units; USDC and stNIBI use 6 decimals. | `{"get_collateral_denom":{}}`, perp `{"get_collateral":{"index":1}}` |
| SLP shares | The receipt/share token minted to LPs. Shares represent a pro-rata claim on SLP Vault accounting value. | `{"get_vault_share_denom":{}}`, `{"vault_snapshot":{}}` |
| Total supply | Total SLP shares outstanding, in base share units. This is queried through the SLP Vault token minter and surfaced in `vault_snapshot`. | `{"vault_snapshot":{}}` |
| Share price | Collateral per SLP share, stored as `share_to_assets_price`. Deposits mint `assets / share_price`; redemptions return `shares * share_price`. | `{"vault_snapshot":{}}`, raw `share_to_assets_price` |
| Collateralization | Policy health ratio. When `acc_pnl_per_token_used` is positive, it is approximately `(1 + acc_rewards_per_token - acc_pnl_per_token_used) / (1 + acc_rewards_per_token)`. Values below `1.0` mean the SLP Vault is below target by contract accounting. | `{"collateralization_p":{}}`, `{"vault_snapshot":{}}` |
| TVL | Notional SLP Vault value using total supply and `1 + acc_rewards_per_token`. It is not the same as immediately available collateral. | `{"tvl":{}}` |
| Market cap / available assets | Current redeemable/accounting value using total supply and `share_to_assets_price`. These queries currently return the same value. | `{"market_cap":{}}`, `{"available_assets":{}}` |
| Net profit | Derived operator metric: `total_rewards - total_closed_pnl - total_liability + current_epoch_positive_open_pnl`. Negative values indicate the SLP Vault is underwater by this accounting view. | `{"get_revenue_info":{}}` |

### Health and PnL State

These variables explain why an SLP Vault is healthy or stressed. They are the
main raw items to check when `collateralization_p`, `share_price`, or deposit
caps look surprising.

| Raw key | Definition | Smart-query surface |
| --- | --- | --- |
| `acc_rewards_per_token` | Per-share reward accumulator. It increases when `distribute_reward` is executed with the SLP Vault's collateral. Raising this improves `share_price` and can improve `collateralization_p`. | `{"vault_snapshot":{}}`, `{"get_revenue_info":{}}` |
| `acc_pnl_per_token` | Raw cumulative PnL per share. This tracks realized and feed-driven PnL at the accounting layer. | Raw state; indirectly reflected in health/revenue queries |
| `acc_pnl_per_token_used` | Policy-applied PnL per share. This is the main stress value for `collateralization_p` and the deposit cap. If it is positive, the SLP Vault is carrying used trader profit / liability pressure. | `{"collateralization_p":{}}`, raw state |
| `share_to_assets_price` | Current collateral-per-share price used for minting and redeeming. It is recomputed from rewards and used PnL. | `{"vault_snapshot":{}}`, `{"market_cap":{}}`, raw state |
| `total_rewards` | Cumulative collateral distributed as SLP Vault rewards through `distribute_reward`. This is historical/accounted rewards, not a pending reward queue. | `{"get_revenue_info":{}}`, `{"vault_snapshot":{}}` |
| `total_closed_pnl` | Cumulative realized trader PnL from closed trades. Positive values mean the SLP Vault has paid trader profits net of received losses. | `{"get_revenue_info":{}}`, raw state |
| `total_liability` | Total tracked SLP Vault obligations. Higher liability worsens `net_profit`. | `{"get_revenue_info":{}}`, `{"vault_snapshot":{}}` |
| `current_epoch_positive_open_pnl` | Positive open trader PnL tracked for the current epoch. It is used by `get_revenue_info` and epoch accounting. | `{"get_revenue_info":{}}`, raw state |

Query the operator view:

```bash
sai_q "$VAULT_G0_USDC" '{"vault_snapshot":{}}'
sai_q "$VAULT_G0_USDC" '{"get_revenue_info":{}}'
sai_q "$VAULT_G0_USDC" '{"collateralization_p":{}}'
```

Query the backing raw items:

```bash
for key in \
  acc_rewards_per_token \
  acc_pnl_per_token \
  acc_pnl_per_token_used \
  share_to_assets_price \
  total_rewards \
  total_closed_pnl \
  total_liability \
  current_epoch_positive_open_pnl
do
  raw_item "$VAULT_G0_USDC" "$key"
done
```

### Daily Risk Params

Use SLP Vault smart query `{"risk_params":{}}` as the main operator view for
daily payout limits and daily PnL throttle state.

```bash
sai_q "$VAULT_G0_USDC" '{"risk_params":{}}' | jq '.data'
```

Important response fields:

| Field | Meaning |
| --- | --- |
| `collateral_denom` | Native collateral denom accepted by the SLP Vault. |
| `balance_now` | Current bank balance of the SLP Vault contract for `collateral_denom`, in base units. |
| `daily_balance_snapshot` | Bank balance captured at the most recent daily reset. The balance-based daily payout cap uses this snapshot, not `balance_now`. |
| `max_daily_balance_loss_pct` | Configured maximum fraction of `daily_balance_snapshot` that can be paid out through trader-profit settlement in one daily window. |
| `daily_payout_cap` | Maximum collateral amount that can be paid out in the current daily window: `daily_balance_snapshot * max_daily_balance_loss_pct`. |
| `daily_payout_used` | Collateral already paid out through the capped settlement path in the current daily window. |
| `daily_payout_remaining` | Remaining collateral payout capacity for the current daily window, floored at zero. |
| `last_daily_acc_pnl_delta_reset` | Timestamp of the most recent daily reset, encoded as nanoseconds in JSON. The next reset is eligible after this timestamp plus 86,400 seconds. |
| `max_daily_acc_pnl_delta` | Daily cap on positive realized PnL movement, expressed per share as a decimal. |
| `daily_acc_pnl_delta` | Accumulated realized PnL movement for the current daily window, expressed per share as a signed decimal. |
| `max_acc_open_pnl_delta` | Per-epoch cap on positive open PnL movement used by the open-trades PnL feed. |
| `acc_pnl_per_token` | Raw cumulative PnL accumulator per SLP share. |
| `acc_pnl_per_token_used` | Policy-applied PnL accumulator per SLP share. This value drives `share_to_assets_price` and `collateralization_p`. |
| `acc_rewards_per_token` | Cumulative distributed reward accumulator per SLP share. |
| `max_acc_pnl_per_token` | Maximum PnL accumulator allowed by rewards: `1 + acc_rewards_per_token`. |
| `share_to_assets_price` | Current collateral-per-share price used for mint and redeem accounting. |

Daily resets are lazy. Passing the `last_daily_acc_pnl_delta_reset + 86,400`
timestamp does not update storage by itself. The contract refreshes
`daily_acc_pnl_delta`, `daily_balance_snapshot`, `daily_payout_used`, and
`last_daily_acc_pnl_delta_reset` when an eligible execution path calls the daily
reset helper, such as SLP Vault settlement function `send_assets` or
`receive_assets`. SLP Vault execute message `{"distribute_reward":{}}` increases
`balance_now`, `total_rewards`, and `acc_rewards_per_token`, but it does not
refresh `daily_balance_snapshot`.

### Deposit Cap State

Large SLP deposits can fail even when the user has enough collateral. The cap is
active when `acc_pnl_per_token_used > 0`. In that state, the SLP Vault limits new
share minting to the remaining headroom under `current_max_supply`.

| Raw key / config field | Definition | Where to query |
| --- | --- | --- |
| `current_max_supply` | Current maximum allowed SLP share supply while the cap is active. Remaining mint headroom is `current_max_supply - total_supply`, floored at zero. | Raw state; effect surfaced through `{"max_mint":{}}` |
| `last_max_supply_update` | Timestamp of the last `current_max_supply` refresh. The refresh is gated to at most once every 86,400 seconds on eligible SLP Vault flows. | Raw state |
| `max_supply_increase_daily_p` | Configured daily supply growth percentage. New cap is `total_supply * (1 + max_supply_increase_daily_p)` when the refresh runs. | `{"config":{}}` |
| `max_mint` | Derived maximum number of SLP shares that can be minted right now. Returns `u128::MAX` when the cap is effectively off. | `{"max_mint":{}}` |
| `max_deposit` | Derived maximum collateral deposit right now, equal to `max_mint` converted through `share_to_assets_price`. | `{"max_deposit":{}}` |

Operator queries:

```bash
sai_q "$VAULT_G0_USDC" '{"max_deposit":{}}'
sai_q "$VAULT_G0_USDC" '{"max_mint":{}}'
sai_q "$VAULT_G0_USDC" '{"config":{}}'
raw_item "$VAULT_G0_USDC" current_max_supply
raw_item "$VAULT_G0_USDC" last_max_supply_update
```

### Epoch and Withdrawal State

Epoch state affects withdrawal request timing and the open/locked window for
withdrawals. Epoch starts are stored as nanosecond timestamps.

| Raw key / query | Definition | Where to query |
| --- | --- | --- |
| `current_epoch` | Current SLP Vault epoch number. | `{"current_epoch":{}}`, raw state |
| `current_epoch_start` | Timestamp when the current epoch started. | `{"get_current_epoch_start":{}}`, raw state |
| `withdraw_lock_thresholds_p` | Collateralization bands that determine whether withdrawals wait 1, 2, or 3 epochs. | `{"config":{}}` |
| `withdraw_epochs_timelock` | Derived number of epochs a new withdrawal request must wait at current collateralization. | `{"withdraw_epochs_timelock":{}}` |

Operator queries:

```bash
sai_q "$VAULT_G0_USDC" '{"current_epoch":{}}'
sai_q "$VAULT_G0_USDC" '{"get_current_epoch_start":{}}'
sai_q "$VAULT_G0_USDC" '{"withdraw_epochs_timelock":{}}'
sai_q "$VAULT_G0_USDC" '{"config":{}}'
```

#### Check Epoch and Daily Reset Distance

Use this when asking "when is the next SLP reset?" or "how far away is the
epoch time?". It compares the current machine time from `date` against on-chain
vault timestamps.

```bash
VAULT="$VAULT_G0_USDC" # or any SLP vault address

epoch_ns="$(sai_q "$VAULT" '{"get_current_epoch_start":{}}' \
  | jq -r '.data')"
current_epoch="$(sai_q "$VAULT" '{"current_epoch":{}}' | jq -r '.data')"

raw_json_item() {
  contract="$1"
  key="$2"
  hex="$(printf '%s' "$key" | xxd -p -c 256)"
  nibid query wasm contract-state raw "$contract" "$hex" |
    jq -r '.data // empty' | base64 -d | jq -r .
}

daily_reset_ns="$(raw_json_item "$VAULT" last_daily_acc_pnl_delta_reset)"
supply_update_ns="$(raw_json_item "$VAULT" last_max_supply_update)"

python3 - "$current_epoch" "$epoch_ns" "$daily_reset_ns" "$supply_update_ns" <<'PY'
import sys, time
from datetime import datetime, timezone

current_epoch = sys.argv[1]
epoch_ns = int(sys.argv[2])
daily_ns = int(sys.argv[3])
supply_ns = int(sys.argv[4])
now = int(time.time())

def fmt(ns):
    return datetime.fromtimestamp(ns // 1_000_000_000, tz=timezone.utc)

def away(sec):
    delta = sec - now
    suffix = "away" if delta >= 0 else "ago"
    delta = abs(delta)
    return f"{delta // 3600}h {(delta % 3600) // 60}m {delta % 60}s {suffix}"

rows = [
    ("current_epoch", current_epoch, None),
    ("epoch_start", fmt(epoch_ns), epoch_ns // 1_000_000_000),
    ("withdraw_open_ends", fmt(epoch_ns + 48 * 3600 * 1_000_000_000), (epoch_ns // 1_000_000_000) + 48 * 3600),
    ("next_epoch_boundary", fmt(epoch_ns + 72 * 3600 * 1_000_000_000), (epoch_ns // 1_000_000_000) + 72 * 3600),
    ("next_daily_pnl_reset_eligible", fmt(daily_ns + 86400 * 1_000_000_000), (daily_ns // 1_000_000_000) + 86400),
    ("next_supply_cap_refresh_eligible", fmt(supply_ns + 86400 * 1_000_000_000), (supply_ns // 1_000_000_000) + 86400),
]

print("local_now:", datetime.now().astimezone())
print("utc_now:", datetime.now(timezone.utc))
for label, value, sec in rows:
    print(f"{label}: {value}" + (f" ({away(sec)})" if sec else ""))
PY
```

### Reward Funding State

There is no separate pending SLP Vault reward queue. Rewards become SLP Vault
accounting only when collateral is sent with `{"distribute_reward":{}}`.

| State | Definition | Where to query |
| --- | --- | --- |
| `total_rewards` | Cumulative rewards already distributed to the SLP Vault. | `{"get_revenue_info":{}}`, raw `total_rewards` |
| `acc_rewards_per_token` | Per-share form of distributed rewards. | `{"vault_snapshot":{}}`, raw `acc_rewards_per_token` |
| perp `pending_gov_fees` | Governance fees accrued on the perp contract by collateral. These are not SLP Vault rewards until claimed and explicitly distributed to an SLP Vault. | perp `{"get_pending_gov_fees":{"index":1}}`, raw perp map |
| `vault_closing_fee_p` | Perp config that controls what portion of closing fees routes directly to the SLP Vault reward path during trading. | perp fee/config queries or raw state |
| `vault_addresses` | Perp map from `(GroupIndex, TokenIndex)` to target SLP Vault address. | perp `{"get_vault_address":{...}}` |

Operator queries:

```bash
sai_q "$VAULT_G0_USDC" '{"get_revenue_info":{}}'
sai_q "$PERP" '{"get_pending_gov_fees":{"index":1}}' # USDC
sai_q "$PERP" '{"get_pending_gov_fees":{"index":2}}' # stNIBI
sai_q "$PERP" '{"get_vault_address":{"group_index":"GroupIndex(0)","collateral_index":"TokenIndex(1)"}}'
```

### Config and Denom State

Use these values to interpret units, risk limits, and which collateral must be
attached to executes.

| Key / field | Definition | Where to query |
| --- | --- | --- |
| `collateral_denom` | Native denom accepted by the SLP Vault for deposits and `distribute_reward`. | `{"get_collateral_denom":{}}` |
| `vault_denom` | Native denom of the SLP share token. | `{"get_vault_share_denom":{}}` |
| `vault_token_minter_contract` | Contract queried for total SLP share supply and used to mint/burn SLP shares. | raw state or config/source |
| `max_daily_acc_pnl_delta` | Daily throttle on positive realized PnL payout accounting. | `{"config":{}}` |
| `max_acc_open_pnl_delta` | Cap applied to positive open PnL updates per epoch. | `{"config":{}}` |
| `losses_burn_p` | Portion of incoming trader-loss assets reserved for deficit recovery under specific negative-PnL conditions. | `{"config":{}}` |
| `max_discount_p` | Maximum locked-deposit bonus/discount. | `{"config":{}}` |
| `max_discount_threshold_p` | Collateralization threshold where locked-deposit bonus phases out. | `{"config":{}}` |
| `min_lock_duration` | Minimum lock duration accepted for locked deposits. | `{"config":{}}` |

Operator queries:

```bash
sai_q "$VAULT_G0_USDC" '{"config":{}}'
sai_q "$VAULT_G0_USDC" '{"get_collateral_denom":{}}'
sai_q "$VAULT_G0_USDC" '{"get_vault_share_denom":{}}'
```

## Health Snapshot

Start here when diagnosing whether an SLP Vault is healthy or why LP deposits are
blocked.

```bash
sai_q "$VAULT_G0_USDC" '{"vault_snapshot":{}}'
sai_q "$VAULT_G0_USDC" '{"get_revenue_info":{}}'
sai_q "$VAULT_G0_USDC" '{"collateralization_p":{}}'
sai_q "$VAULT_G0_USDC" '{"available_assets":{}}'
sai_q "$VAULT_G0_USDC" '{"market_cap":{}}'
sai_q "$VAULT_G0_USDC" '{"tvl":{}}'
```

Important fields:

- `collateralization_p`: Policy health ratio. Values below `1.0` mean the SLP Vault
  is under target by the contract's accounting.
- `share_price`: Current collateral-per-share price used for mint/redeem.
- `total_supply`: Total SLP share supply in base share units.
- `total_liability`: SLP Vault obligations tracked by the contract.
- `total_rewards`: Rewards already distributed through `DistributeReward`.
- `current_epoch_positive_open_pnl`: Positive open PnL tracked for the current
  epoch.

`get_revenue_info` computes:

```text
net_profit = rewards - closed_pnl - liabilities + current_epoch_positive_open_pnl
```

## Deposit Caps

Use these when users report that large SLP deposits are failing.

```bash
sai_q "$VAULT_G0_USDC" '{"max_deposit":{}}'
sai_q "$VAULT_G0_USDC" '{"max_mint":{}}'
sai_q "$VAULT_G0_USDC" '{"config":{}}'
```

`max_deposit` is returned in collateral base units. For USDC and stNIBI, divide
by `1e6` for human units.

When `acc_pnl_per_token_used > 0`, deposits are capped by remaining mint
headroom under `current_max_supply`. `current_max_supply` is refreshed at most
once per 86,400 seconds on eligible SLP Vault flows and uses:

```text
current_max_supply = total_supply * (1 + max_supply_increase_daily_p)
```

When `acc_pnl_per_token_used <= 0`, `max_mint` and `max_deposit` return
`u128::MAX`, meaning this cap is effectively off.

## Rewards and Collateralization

Do not plain-send collateral to the SLP Vault address when trying to improve health.
A bank transfer can increase the contract's balance, but it will not update
SLP Vault accounting.

To intentionally increase `acc_rewards_per_token`, `total_rewards`,
`share_price`, and `collateralization_p`, execute `distribute_reward` with the
SLP Vault's collateral denom:

```bash
USDC_DENOM="erc20/0x0829F361A05D993d5CEb035cA6DF3446b060970b"

nibid tx wasm execute "$VAULT_G0_USDC" \
  '{"distribute_reward":{}}' \
  --amount "1000000000$USDC_DENOM" \
  --from <your-key> \
  --gas 300000
```

The example above distributes `1,000` USDC to all SLP holders. This does not mint
new shares. It raises the per-share reward accumulator.

To estimate the reward needed for a target collateralization:

```text
max = 1 + acc_rewards_per_token
used = acc_pnl_per_token_used
collateralization_p = (max - used) / max

target_reward_per_share = used / (1 - target_collateralization_p) - max
target_reward_amount = target_reward_per_share * total_supply
```

This formula assumes `acc_pnl_per_token_used` is positive.

## Pending Governance Fees

Perp-side `pending_gov_fees` are not automatically SLP rewards. They are
admin-controlled accounting on the perp contract. The admin can claim them to a
recipient, and ops can then execute `distribute_reward` on the target SLP Vault if
that is the intended use.

Smart queries:

```bash
# TokenIndex(1) = USDC, TokenIndex(2) = stNIBI
sai_q "$PERP" '{"get_pending_gov_fees":{"index":1}}'
sai_q "$PERP" '{"get_pending_gov_fees":{"index":2}}'
sai_q "$PERP" '{"get_collateral":{"index":1}}'
sai_q "$PERP" '{"get_collateral":{"index":2}}'
```

Do not use `UpdatePendingGovFees` as a funding mechanism. It is an admin state
update and does not transfer funds into the SLP Vault.

## Raw SLP Vault State

SLP Vault `Item` keys are raw ASCII key hex without a namespace length prefix.

```bash
raw_item() {
  contract="$1"
  key="$2"
  hex=$(printf '%s' "$key" | xxd -p -c 256)
  nibid query wasm contract-state raw "$contract" "$hex" |
    jq -r '.data // empty' | base64 -d
  echo
}

for key in \
  total_rewards \
  total_closed_pnl \
  total_liability \
  current_epoch_positive_open_pnl \
  acc_rewards_per_token \
  acc_pnl_per_token \
  acc_pnl_per_token_used \
  share_to_assets_price
do
  echo "=== $key ==="
  raw_item "$VAULT_G0_USDC" "$key"
done
```

Useful raw keys:

| Key | Meaning |
| --- | --- |
| `total_rewards` | Cumulative rewards distributed to the SLP Vault |
| `total_closed_pnl` | Cumulative realized trader PnL paid/received |
| `total_liability` | Total tracked SLP Vault obligations |
| `current_epoch_positive_open_pnl` | Positive open PnL for current epoch |
| `acc_rewards_per_token` | Per-share reward accumulator |
| `acc_pnl_per_token` | Raw cumulative PnL per share |
| `acc_pnl_per_token_used` | Policy-applied PnL per share |
| `share_to_assets_price` | Collateral per share for mint/redeem |

## Raw Pending Gov Fees

`PENDING_GOV_FEES` is a perp contract map:

```rust
Map<TokenIndex, Uint128>::new("pending_gov_fees")
```

`TokenIndex` is a two-byte big-endian `u16`. For a single-key map, use:

```text
len(namespace) || namespace || token_index_be_u16
```

Raw query helper:

```bash
raw_pending_gov_fees() {
  idx="$1"
  ns="pending_gov_fees"
  ns_hex=$(printf '%s' "$ns" | xxd -p -c 256)
  ns_len_hex=$(printf '%04x' "${#ns}")
  idx_hex=$(printf '%04x' "$idx")
  hex_key="${ns_len_hex}${ns_hex}${idx_hex}"

  nibid query wasm contract-state raw "$PERP" "$hex_key" |
    jq -r '.data // empty' | base64 -d
  echo
}

raw_pending_gov_fees 1 # USDC
raw_pending_gov_fees 2 # stNIBI
```

## Epoch and Withdrawal Timing

```bash
sai_q "$VAULT_G0_USDC" '{"current_epoch":{}}'
sai_q "$VAULT_G0_USDC" '{"get_current_epoch_start":{}}'
sai_q "$VAULT_G0_USDC" '{"withdraw_epochs_timelock":{}}'
```

Epoch start is a `Timestamp` nanosecond value. Convert with:

```bash
python3 - <<'PY'
from datetime import datetime, timezone
ns = 1778544218237604327
print(datetime.fromtimestamp(ns / 1e9, tz=timezone.utc))
PY
```
