# Sai contract queries

Use this reference for read-only live queries of Sai mainnet Perp, Oracle, and
SLP Vault contracts through `nibid` or the TypeScript SDK. It covers markets,
collaterals, `MarketIndex`, `TokenIndex`, `GroupIndex`, open interest and
limits, fee configuration, vault mappings, referral ownership, deposits, and
trading credits. Use agent skill `nibiru-cli-nibid` for CLI mechanics.

## Contract query playbook

Source of truth for deployed query examples: file
`$HOME/ki/sai-website/webapp/pages/debug.tsx` (`QUERY_CONFIG`). The Rust
`QueryMsg` enums in the contract repositories are authoritative for the
supported message names and response types.
Full validated CLI notes: `$HOME/ki/boku/epics/26-02-10-sai-mainnet-query.md`.

### Mainnet addresses

| Contract | Address |
|----------|---------|
| **Perp** | `nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph` |
| **Oracle** | `nibi1xfwyfwtdame6645lgcs4xvf4u0hpsuvxrcelfwtztu0pv7n4l6hqw5a8gj` |
| SLP-USDC Vault — Group 0 | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault — Group 0 | `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej` |
| SLP-USDC Vault — Group 1 | `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8` |
| SLP-stNIBI Vault — Group 1 | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |

Source: `$HOME/ki/sai-website/webapp/config/env.ts` (`SaiContractsMainnet`).

Group meanings:
- `GroupIndex(0)` = crypto assets
- `GroupIndex(1)` = real estate
- `GroupIndex(2)` = exotic
- `GroupIndex(3)` = watch assets
- `GroupIndex(4)` = equities and commodities

Vault mappings are shared across some groups. Query `get_vault_address` for the
exact group/collateral pair when in doubt. Mainnet currently maps:
- Group 0: USDC vault `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p`; stNIBI vault `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej`
- Group 1: USDC vault `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8`; stNIBI vault `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj`
- Group 2: USDC vault `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p`; stNIBI vault `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj`
- Group 3: USDC vault `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8`; stNIBI vault `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj`
- Group 4: USDC vault `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p`; stNIBI vault `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej`

## Related references and skills

- [`sai-graphql.md`](sai-graphql.md): indexed, app-facing trades, vaults,
  prices, referrals, and subscriptions; use it instead of contract queries when
  the question is what Sai Keeper exposes.
- Agent skill `sai-db`: private database connectivity, primary/replica routing,
  and write workflows. This bundle's [`sai-db.md`](sai-db.md) covers static
  schema and query semantics.
- [`sai-rest.md`](sai-rest.md): public aggregate metrics and health checks.
- Agent skill `nibiru-cli-nibid`: command mechanics for
  `nibid query wasm contract-state <raw|smart>`. This reference supplies Sai
  addresses and payloads.

## CLI setup

```bash
PERP="nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph"
ORACLE="nibi1xfwyfwtdame6645lgcs4xvf4u0hpsuvxrcelfwtztu0pv7n4l6hqw5a8gj"
# helper alias
sai_perps_q() { nibid query wasm contract-state smart "$1" "$2"; }
```

## Rules of thumb

- Query keys are **snake_case** (CosmWasm serde). `GetMarket` → `get_market`.
- Some args use **wrapped string indices**: `"index": "MarketIndex(0)"`, `"index": "GroupIndex(0)"`, `"group_index": "GroupIndex(1)"`, `"collateral_index": "TokenIndex(1)"`.
- Some args use **plain integers**: `collateral_index: 1`, `market_index: 0`, `group_index: 0`.
  - When both arg names are plain integers, use integers. When the field name takes a typed index (e.g. `get_vault_address`), use the wrapped string form.
  - Check `QUERY_CONFIG` in file `pages/debug.tsx` for deployed examples.
- **Never use `collateral_index: 0`**. On mainnet `TokenIndex(0)` is the quote/USD placeholder — not a collateral. Valid collaterals are `TokenIndex(1)` (USDC) and `TokenIndex(2)` (stNIBI). Confirm first with `list_collaterals`.
- OI values (`long`, `short`, `max`) are in **collateral token base units** and represent **position size (margin × leverage)**, not margin only. USDC and stNIBI both have 6 decimals → divide by `1e6` to get human units.

## Discovery sequence

Run these first when you need to understand available markets/groups/collaterals:

```bash
sai_perps_q "$PERP" '{"list_markets":{}}'
sai_perps_q "$PERP" '{"list_groups":{}}'
sai_perps_q "$PERP" '{"list_collaterals":{}}'
sai_perps_q "$ORACLE" '{"list_tokens":{"limit":30}}'
```

`list_tokens` is paginated (default 10, max 30). For >30 tokens use `"start_after": <last_id>`.

To map a market to its group:

```bash
sai_perps_q "$PERP" '{"get_market":{"index":"MarketIndex(16)"}}'
# → {"data":{"base":"TokenIndex(17)","quote":"TokenIndex(0)","group_index":"GroupIndex(0)",...}}
```

## Collaterals (mainnet)

| TokenIndex | Name |
|-----------|------|
| 1 | USDC |
| 2 | stNIBI (ampNIBI) |

### stNIBI price in USD (mainnet quick path)

On Sai mainnet Oracle, `id: 2` is `stnibi` and `id: 1` is `usdc`. These token IDs
are stable on mainnet and can be reused directly.

```bash
# sanity check (optional)
sai_perps_q "$ORACLE" '{"get_token_by_id":{"id":2}}'
# → {"data":{"id":2,"base":"stnibi",...}}

# direct stNIBI/USD oracle price
sai_perps_q "$ORACLE" '{"get_price":{"index":2}}'

# optional cross-check against USDC and ratio
sai_perps_q "$ORACLE" '{"get_price":{"index":1}}'
sai_perps_q "$ORACLE" '{"get_exchange_rate":{"base":2,"quote":1}}'
```

## Common MarketIndex → asset (mainnet)

| MarketIndex | Base |
|-------------|------|
| 0 | BTC |
| 1 | ETH |
| 16 | SOL |
| 17 | XRP |
| 18 | SUI |
| 29 | BNB |
| 48 | NIBI |
| 49 | POKEMON |
| 53 | AUDEMARS |
| 1001 | SPY |
| 1002 | NVDA |
| 1006 | AAPL |

The full `TokenIndex` and `MarketIndex` lookup appears later in this file under
"Mainnet tokens and markets reference." Do not assume all non-real-estate
markets are `GroupIndex(0)`; mainnet also has exotic, watch, and
equities/commodities groups.

## OI queries

Two layers of limits. A trade is rejected if **either** is exceeded (`ExposureLimitReached`).

```bash
# Pair OI — limit for a specific market
sai_perps_q "$PERP" '{"get_borrowing_pair_oi":{"collateral_index":1,"market_index":0}}'
# → {"data":{"long":"...","short":"...","max":"10000000000000"}}

# Group OI — shared limit across all markets in the group
sai_perps_q "$PERP" '{"get_borrowing_group_oi":{"collateral_index":1,"group_index":0}}'
# → {"data":{"long":"...","short":"...","max":"10000000000"}}
```

Recorded mainnet snapshot (Group 0 + USDC): pair max = 10,000,000 USDC, group max = 10,000 USDC. Group is usually the binding constraint.

## Fee config validation

Validate deployed fee parameters with on-chain queries (`get_fees`,
`get_fee_tiers`, `get_trader_fee_multiplier`, `get_pending_gov_fees`). Expected
defaults, tier table, validation checklist, and settings that are **not**
queryable on-chain are in
[sai-contracts.md § Fee config validation](#fee-config-validation-1).

Fee semantics (what fees mean, distribution, code paths):
`/epics/26-05-26-sai-perpetuals-fee-analysis.md`.

## Curated query reference

Query message JSON templates, organized by contract. Validate against the
contract `QueryMsg` enum when the web-app catalog and a deployed contract could
be on different release cadences.

## Perp contract

Below are smart query request payloads for the Sai perp contract. 
You can use the `/nibiru-cli-nibid` skill to pull any of this information.

- Source: sai-perps repo contracts/perp for the Rust logic

```json
{"list_markets":{}}
{"list_groups":{}}
{"list_collaterals":{}}
{"get_market":{"index":"MarketIndex(0)"}}
{"get_group":{"index":"GroupIndex(0)"}}
{"get_collateral":{"index":1}}
{"get_fees":{"index":"FeeIndex(0)"}}
{"get_pair_custom_max_leverage":{"index":0}}
{"get_after_hours_leverage":{}}
{"is_after_hours_market":{"market_index":"MarketIndex(0)"}}
{"list_after_hours_markets":{}}
{"get_borrowing_pair":{"collateral_index":1,"market_index":0}}
{"get_borrowing_pair_oi":{"collateral_index":1,"market_index":0}}
{"get_borrowing_pair_group":{"collateral_index":1,"market_index":0}}
{"get_borrowing_group":{"collateral_index":1,"group_index":0}}
{"get_borrowing_group_oi":{"collateral_index":1,"group_index":0}}
{"get_vault_address":{"group_index":"GroupIndex(0)","collateral_index":"TokenIndex(1)"}}
{"get_trade":{"trader":"nibi1...","index":0}}
{"get_trade_raw":{"trader":"nibi1...","index":0}}
{"get_trades":[["nibi1...",0],["nibi1...",1]]}
{"list_trades_for_user":{"trader":"nibi1...","show_closed":false,"show_conditional":false,"scan_limit":100,"start_idx":null}}
{"get_trade_info":{"trader":"nibi1...","index":0}}
{"get_trade_infos":{"trader":"nibi1..."}}
{"get_trade_pnl":{"trader":"nibi1...","index":0}}
{"get_liquidation_price":{"trade_id":"UserTradeIndex(0)","trader":"nibi1...","include_borrowing_fees":true}}
{"get_perp_prices":{"market_index":"MarketIndex(1)","collateral_index":"TokenIndex(1)"}}
{"get_trader_fee_multiplier":{"trader":"nibi1..."}}
{"is_trader_stored":{"trader":"nibi1..."}}
{"get_fee_tiers":{}}
{"get_fee_tier_min_hold_blocks":{}}
{"get_short_lived_penalty_curve":{}}
{"get_short_lived_penalty_rate":{"market_index":0,"hold_blocks":0}}
{"get_short_lived_penalty_f0_for_market":{"market_index":0}}
{"get_pending_gov_fees":{"index":0}}
{"get_oracle_address":{}}
{"get_trading_activated":{}}
{"get_market_trading_state":{"market_index":"MarketIndex(0)"}}
{"get_collateral_price":{"collateral_index":"TokenIndex(1)","vault_address":null}}
{"get_trading_credit_params":{}}
{"get_affiliate_claimable_rewards":{"address":"nibi1..."}}
{"get_oi_windows_settings":{}}
{"get_windows":{"windows_duration":3600,"market_index":0,"current_window_id":0}}
{"get_pair_depth":{"index":0}}
{"get_user_deposit":{"user":"nibi1...","collateral_index":1}}
{"list_user_deposits":{"user":"nibi1..."}}
```

`get_trade` returns a complete `TradeData` record. `get_trade_raw` returns only
the stored `Trade`. `get_trades` is the batch form of `get_trade`: it preserves input order
and duplicate keys, returns `null` for missing keys, and returns
`Vec<Option<TradeData>>` for present keys. Each `TradeData` includes `trade`,
`trade_info`, `initial_acc_fees`, `liquidation_price`, and optional
`needs_after_hours_trigger`, so it is suitable for keeper batch trade refreshes
that need open/close metadata and fee snapshots.

## SLP Vault contract

For SLP Vault operations, health accounting, reward distribution, raw state keys,
and mainnet runbooks, see [sai-vaults.md](sai-vaults.md).
For epoch and reset timing, see
[Check Epoch and Daily Reset Distance](sai-vaults.md#check-epoch-and-daily-reset-distance).

You can use the `/nibiru-cli-nibid` skill to pull any of this information.

- Source: sai-perps repo contracts/vault for the Rust logic

### SLP Vault smart queries

- `{"tvl":{}}` - Notional SLP Vault value using total supply and 
  `1 + acc_rewards_per_token`. Returns a `Uint128` collateral base-unit amount.
- `{"available_assets":{}}` - Current accounting value available at 
  `share_to_assets_price`. Returns a `Uint128` collateral base-unit amount.
- `{"market_cap":{}}` - SLP share market-cap style value, 
  `total_supply * share_to_assets_price`. Returns a `Uint128` collateral base-unit amount.
- `{"current_epoch":{}}` - Current SLP Vault epoch number. Returns a `u64`.
- `{"get_current_epoch_start":{}}` - Current epoch start timestamp. 
  Returns a `Timestamp` encoded as a nanosecond string in JSON.
- `{"config":{}}` - SLP Vault addresses and risk parameters. 
  Returns `ConfigResponse` with manager/admin addresses, perp/oracle/feed
  addresses, PnL limits, daily supply cap, discount params, withdrawal thresholds,
  and `min_lock_duration`.
- `{"risk_params":{}}` - SLP Vault daily risk and payout-cap state. Returns
  `RiskParamsResponse` with current collateral balance, daily balance snapshot,
  daily payout cap and remaining amount, daily PnL throttle state, PnL
  accumulators, and share price. See
  [Daily Risk Params](sai-vaults.md#daily-risk-params).
- `{"vault_snapshot":{}}` - One-call operator health snapshot. Returns
  `VaultSnapshotResponse` with `tvl`, `market_cap`, `share_price`,
  `collateralization_p`, `total_supply`, `total_liability`, `total_rewards`,
  `epoch`, and `epoch_start`.
- `{"get_revenue_info":{}}` - Revenue and PnL accounting summary. 
  Returns `RevenueInfo` with `net_profit`, `rewards`, `closed_pnl`, `liabilities`, and `current_epoch_positive_open_pnl`.
- `{"collateralization_p":{}}` - Policy health ratio for the SLP Vault. 
  Returns a `Decimal`; values below `1.0` mean the SLP Vault is below target by contract accounting.
- `{"withdraw_epochs_timelock":{}}` - Number of epochs a new withdrawal request
  must wait at current collateralization. Returns a `u64`.
- `{"get_vault_share_denom":{}}` - Native denom for the SLP share token. Returns
  a `String`.
- `{"get_collateral_denom":{}}` - Native collateral denom accepted by the SLP
  Vault. Returns a `String`.
- `{"max_mint":{}}` - Maximum SLP shares that can be minted right now under the
  supply cap. Returns a `Uint128` share base-unit amount, or `u128::MAX` when
  uncapped.
- `{"max_deposit":{}}` - Maximum collateral that can be deposited right now.
  Returns a `Uint128` collateral base-unit amount, or `u128::MAX` when uncapped.
- `{"max_redeem":{"depositor":"nibi1..."}}` - Maximum SLP shares a depositor can
  currently redeem from matured withdrawal requests. Returns a `Uint128` share
  base-unit amount.
- `{"max_withdraw":{"depositor":"nibi1..."}}` - Maximum collateral a depositor
  can currently withdraw from matured withdrawal requests. Returns a `Uint128`
  collateral base-unit amount.
- `{"total_shares_being_withdrawn":{"depositor":"nibi1..."}}` - Total SLP shares
  currently recorded in withdrawal requests for a depositor. Returns a `Uint128`
  share base-unit amount.
- `{"user_withdraw_requests":{"user":"nibi1..."}}` - Withdrawal requests for a
  user. Returns `UserWithdrawRequestsResponse` with `requests: [{ shares, unlock_epoch, auto_redeem }]`.
- `{"get_locked_deposit":{"deposit_id":0}}` - Locked deposit details by ID.
  Returns `LockedDeposit` with depositor, shares, deposited assets, discount,
  timestamp, and lock duration.
- `{"all_locked_deposits":{"start_after":null,"limit":null}}` - Paginated locked 
  deposit list. Returns `locked_deposits: [(deposit_id, LockedDeposit)]` (type
`AllLockedDepositsResponse`)
- `{"user_locked_deposits":{"user":"nibi1..."}}` - Locked deposits owned by a user. Returns `UserLockedDepositsResponse` with `locked_deposits: [(deposit_id, LockedDeposit)]`.
- `{"lock_discount_p":{"collat_p":"1.0","lock_duration":0}}` - Simulated lock discount for a collateralization ratio and lock duration. Returns a `Decimal` discount percentage.
- `{"user_vault_state":{"user":"nibi1..."}}` - User-level SLP Vault state bundle. Returns `UserVaultStateResponse` with share balance, estimated assets, pending withdrawals, locked deposits, and max redeem/withdraw values.

## Oracle contract

Below are smart query request payloads for the Sai oracle contract.
You can use the `/nibiru-cli-nibid` skill to pull any of this information.

- Source: sai-perps repo contracts/oracle for the Rust logic

```json
{"list_tokens":{"start_after":null,"limit":null}}
{"get_token_by_id":{"id":3}}
{"get_token_by_name":{"name_raw":"btc"}}
{"get_price":{"index":3}}
{"get_exchange_rate":{"base":3,"quote":4}}
{"get_permission_group":{"group_id":2}}
{"list_permission_groups":{"start_after":null,"limit":null}}
{"expiration_time":{}}
{"ownership":{}}
```

## TypeScript (`NibiruQuerier`) alternative

```typescript
import { Mainnet, NibiruQuerier } from "@nibiruchain/nibijs"

const querier = await NibiruQuerier.connect(Mainnet().endptTm)

const result = await querier.wasmClient.queryContractSmart(
  "nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph",
  { get_borrowing_group_oi: { collateral_index: 1, group_index: 0 } },
)
```

Address constants live in `$HOME/ki/sai-website/webapp/config/env.ts` (`SaiContractsMainnet`).


---

## Mainnet tokens and markets reference

This file is the **full lookup** companion to `SKILL.md`.

- [Mainnet addresses](#mainnet-addresses)
- [Collaterals (mainnet)](#collaterals-mainnet)
- [TokenIndex reference (mainnet)](#tokenindex-reference-mainnet)
- [MarketIndex reference (mainnet)](#marketindex-reference-mainnet)
- [Fee config validation](#fee-config-validation)

### Mainnet addresses

| Contract | Address |
|---|---|
| **Perp** | `nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph` |
| **Oracle** | `nibi1xfwyfwtdame6645lgcs4xvf4u0hpsuvxrcelfwtztu0pv7n4l6hqw5a8gj` |
| SLP-USDC Vault, Group 0 | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault, Group 0 | `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej` |
| SLP-USDC Vault, Group 1 | `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8` |
| SLP-stNIBI Vault, Group 1 | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |
| SLP-USDC Vault, Group 2 | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault, Group 2 | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |
| SLP-USDC Vault, Group 3 | `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8` |
| SLP-stNIBI Vault, Group 3 | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |
| SLP-USDC Vault, Group 4 | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault, Group 4 | `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej` |

**Group meanings (Perp `GroupIndex`):**
- `GroupIndex(0)` = main crypto markets
- `GroupIndex(1)` = real estate (coded estate)
- `GroupIndex(2)` = exotic
- `GroupIndex(3)` = watch assets
- `GroupIndex(4)` = equities and commodities

### Collaterals (mainnet)

Never use `collateral_index: 0`. Mainnet collaterals are:

| TokenIndex | base | Notes |
|---:|---|---|
| 1 | `usdc` | collateral |
| 2 | `stnibi` | collateral (contract denom contains `ampNIBI`) |

---

### `TokenIndex` reference (mainnet)

Per the epic doc, **`TokenIndex(0)` is used by Perp as quote (USD)** and is **not**
present in Oracle (`get_token_by_id` returns "token id 0 does not exist").


| TokenIndex | base | permission_group |
|---:|---|---:|
| 0 | (USD quote placeholder; not in Oracle) | - |
| 1 | `usdc` | 2 |
| 2 | `stnibi` | 2 |
| 3 | `btc` | 2 |
| 4 | `eth` | 2 |
| 5 | `atom` | 2 |
| 6 | `dubai` | 1 |
| 7 | `los-angeles` | 1 |
| 8 | `new-york` | 1 |
| 9 | `chicago` | 1 |
| 10 | `washington` | 1 |
| 11 | `pittsburgh` | 1 |
| 12 | `miami-beach` | 1 |
| 13 | `boston` | 1 |
| 14 | `brooklyn` | 1 |
| 15 | `austin` | 1 |
| 16 | `denver` | 1 |
| 17 | `sol` | 2 |
| 18 | `xrp` | 2 |
| 19 | `sui` | 2 |
| 20 | `ena` | 2 |
| 21 | `arb` | 2 |
| 22 | `trx` | 2 |
| 23 | `apt` | 2 |
| 24 | `pol` | 2 |
| 25 | `ton` | 2 |
| 26 | `ada` | 2 |
| 27 | `ltc` | 2 |
| 28 | `doge` | 2 |
| 29 | `aster` | 2 |
| 30 | `bnb` | 2 |
| 31 | `pump` | 2 |
| 32 | `avax` | 2 |
| 33 | `aave` | 2 |
| 34 | `pengu` | 2 |
| 35 | `link` | 2 |
| 36 | `bonk` | 2 |
| 37 | `shib` | 2 |
| 38 | `trump` | 2 |
| 39 | `near` | 2 |
| 40 | `kaito` | 2 |
| 41 | `mnt` | 2 |
| 42 | `ip` | 2 |
| 43 | `virtual` | 2 |
| 44 | `hype` | 2 |
| 45 | `zec` | 2 |
| 46 | `purr` | 2 |
| 47 | `mon` | 2 |
| 48 | `fartcoin` | 2 |
| 49 | `nibi` | 2 |
| 50 | `pokemon-card-index` | 1 |
| 51 | `watch-index` | 1 |
| 52 | `rolex` | 1 |
| 53 | `patek-phillippe` | 1 |
| 54 | `audemars-piguet` | 1 |
| 55 | `omega` | 1 |
| 56 | `cartier` | 1 |
| 57 | `breitling` | 1 |
| 58 | `tudor` | 1 |
| 59 | `qqq` | 1 |
| 60 | `spy` | 1 |
| 61 | `nvda` | 1 |
| 62 | `amd` | 1 |
| 63 | `intc` | 1 |
| 64 | `msft` | 1 |
| 65 | `aapl` | 1 |
| 66 | `googl` | 1 |
| 67 | `amzn` | 1 |
| 68 | `meta` | 1 |
| 69 | `avgo` | 1 |
| 70 | `tsla` | 1 |
| 71 | `brk-b` | 1 |
| 72 | `coin` | 1 |
| 73 | `jpm` | 1 |
| 74 | `pltr` | 1 |
| 75 | `qbts` | 1 |
| 76 | `qubt` | 1 |
| 77 | `xau-usd` | 1 |
| 78 | `xag-usd` | 1 |
| 79 | `xpt-usd` | 1 |
| 80 | `xpd-usd` | 1 |
| 81 | `wti-usd` | 1 |
| 82 | `xbr-usd` | 1 |
| 83 | `ng-usd` | 1 |
| 84 | `gme` | 1 |

### `MarketIndex` reference (mainnet)

Every market has the following fields.
- `quote: TokenIndex(0)` (USD placeholder)
- `spread_p: 0`
- `fee_index` varies by market

Each market has base, quote (always TokenIndex(0) = USD), spread_p, group_index,
fee_index. Base token names resolved via [TokenIndex
reference](#tokenindex-reference-mainnet) above.
```rust
#[cw_serde]
pub struct MarketInfo {
    pub base: TokenIndex,
    pub quote: TokenIndex,
    pub spread_p: Decimal,
    pub group_index: GroupIndex,
    pub fee_index: FeeIndex,
}
```

All markets have `MarketInfo.quote=TokenIndex(0)` (USD).

| MarketIndex | base (TokenIndex) | base (name) | spread_p | group_index | fee_index |
|---|---|---|---|---|---|
| 0  | 3  | `btc`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 1  | 4  | `eth`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 2  | 6  | `dubai`       | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 3  | 7  | `los-angeles` | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 4  | 8  | `new-york`    | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 5  | 9  | `chicago`     | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 6  | 10 | `washington`  | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 7  | 11 | `pittsburgh`  | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 8  | 12 | `miami-beach` | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 9  | 13 | `boston`      | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 10 | 14 | `brooklyn`    | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 11 | 15 | `austin`      | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 12 | 16 | `denver`      | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 16 | 17 | `sol`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 17 | 18 | `xrp`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 18 | 19 | `sui`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 19 | 20 | `ena`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 20 | 21 | `arb`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 21 | 22 | `trx`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 22 | 23 | `apt`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 23 | 24 | `pol`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 24 | 25 | `ton`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 25 | 26 | `ada`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 26 | 27 | `ltc`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 27 | 28 | `doge`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 28 | 29 | `aster`       | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 29 | 30 | `bnb`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 30 | 31 | `pump`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 31 | 32 | `avax`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 32 | 33 | `aave`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 33 | 34 | `pengu`       | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 34 | 35 | `link`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 35 | 36 | `bonk`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 36 | 37 | `shib`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 37 | 38 | `trump`       | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 38 | 39 | `near`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 39 | 40 | `kaito`       | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 40 | 41 | `mnt`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 41 | 42 | `ip`          | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 42 | 43 | `virtual`     | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 43 | 44 | `hype`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 44 | 45 | `zec`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 48 | 49 | `nibi`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 49 | 50 | `pokemon-card-index` | 0 | `GroupIndex(2)` | `FeeIndex(1)` |
| 50 | 51 | `watch-index` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 51 | 52 | `rolex` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 52 | 53 | `patek-phillippe` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 53 | 54 | `audemars-piguet` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 54 | 55 | `omega` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 55 | 56 | `cartier` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 56 | 57 | `breitling` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 57 | 58 | `tudor` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 1000 | 59 | `qqq` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1001 | 60 | `spy` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1002 | 61 | `nvda` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1003 | 62 | `amd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1004 | 63 | `intc` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1005 | 64 | `msft` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1006 | 65 | `aapl` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1007 | 66 | `googl` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1008 | 67 | `amzn` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1009 | 68 | `meta` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1010 | 69 | `avgo` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1011 | 70 | `tsla` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1012 | 71 | `brk-b` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1013 | 72 | `coin` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1014 | 73 | `jpm` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1015 | 74 | `pltr` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1016 | 75 | `qbts` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1017 | 76 | `qubt` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1018 | 77 | `xau-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1019 | 78 | `xag-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1020 | 79 | `xpt-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1021 | 80 | `xpd-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1022 | 81 | `wti-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1023 | 82 | `xbr-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1024 | 83 | `ng-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1025 | 84 | `gme` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |

---

### Fee configuration validation

Use on-chain smart queries to verify deployed fee parameters. For fee
**semantics** (what each fee means, distribution splits, code paths), see
`/epics/26-05-26-sai-perpetuals-fee-analysis.md`.

Use `sai_perps_q` from `SKILL.md`. Index wrappers must match mainnet:
`"MarketIndex(0)"`, `"FeeIndex(0)"`, not `{"0": 0}`.

#### Effective fee formula

```txt
effective_fee = base_fee × min(tier_multiplier, referral_discount_multiplier)
```

Query `get_trader_fee_multiplier` for the combined result for a specific trader.

#### Default base fees (`FeeIndex(0)`)

| Field | Expected default |
|---|---|
| `open_fee_p` | `"0.02"` (2.0%) |
| `close_fee_p` | `"0.015"` (1.5%) |
| `trigger_order_fee_p` | `"0.03"` (3.0%) |
| `min_position_size_usd` | `"1"` |

Resolve the fee index from the market first:

```bash
sai_perps_q "$PERP" '{"get_market":{"index":"MarketIndex(0)"}}'
# → fee_index: "FeeIndex(0)" (most markets)

sai_perps_q "$PERP" '{"get_fees":{"index":"FeeIndex(0)"}}'
```

#### Default fee tiers (`get_fee_tiers`)

Eight tiers; multipliers decrease as point thresholds increase:

| Tier | `points_treshold` | `fee_multiplier` |
|---:|---:|---:|
| 0 | 6_000_000 | 0.975 |
| 1 | 20_000_000 | 0.950 |
| 2 | 50_000_000 | 0.925 |
| 3 | 100_000_000 | 0.900 |
| 4 | 250_000_000 | 0.850 |
| 5 | 400_000_000 | 0.800 |
| 6 | 1_000_000_000 | 0.700 |
| 7 | 2_000_000_000 | 0.600 |

```bash
sai_perps_q "$PERP" '{"get_fee_tiers":{}}'
sai_perps_q "$PERP" '{"get_trader_fee_multiplier":{"trader":"nibi1..."}}'
# → "1.0" (no discount), "0.95" (referral), or lower (tier)
sai_perps_q "$PERP" '{"get_pending_gov_fees":{"index":1}}'  # USDC
```

#### Validation checklist

- [ ] **Base fees:** `get_fees` for each distinct `fee_index` in use
  (`FeeIndex(0)` crypto/equities, `FeeIndex(1)` exotic/watch).
- [ ] **Fee tiers:** `get_fee_tiers` returns 8 tiers with expected defaults.
- [ ] **Trader multipliers:** `get_trader_fee_multiplier` for a new trader
  (`1.0`), referred trader (`< 1.0`), and high-volume trader (tier discount).
- [ ] **Market wiring:** each `get_market` response has a valid `fee_index`.

#### Not queryable on-chain (workarounds)

These live in contract state but have no smart query today. Infer from code
defaults, admin events, or controlled test trades.

| Setting | Storage key | Default | Workaround |
|---|---|---|---|
| Vault closing fee % | `VAULT_CLOSING_FEE_P` | 4.2% of closing-fee component | Close a trade; compare vault reward to closing fee charged |
| Referrer fee tiers | `REFERRER_FEE_PERCENTAGE` | 5%, 10%, 15%, 50% | Code default in `contracts/perp/src/fees/state.rs` |
| Referee discount | `REFERREE_BASE_FEE_MULTIPLIER` | 5% off base (`0.95` effective) | Compare `get_trader_fee_multiplier` with/without referrer |
| Referrer maps | `USER_REFERRERS`, `REFERRER_FEE_TIER`, `REFERRER_FEES` | — | Use `get_trader_fee_multiplier`; indexed referral history via [`sai-graphql.md`](sai-graphql.md) |

See also `slp-vaults.md` for `get_pending_gov_fees` and vault reward routing.
