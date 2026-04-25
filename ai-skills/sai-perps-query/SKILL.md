---
name: sai-perps-query
description: >-
  Run read-only queries against Sai mainnet CosmWasm contracts (Perp, Oracle,
  Vault) using Nibiru CLI (nibid) or the Nibiru TypeScript SDK. Use when the user
  asks to query Sai perp contracts, inspect open interest, check OI limits, look up
  MarketIndex/TokenIndex/GroupIndex, run get_borrowing_group_oi or
  get_borrowing_pair_oi, call queryContractSmart, or inspect Sai markets and
  collaterals.
metadata:
  tags: ["sai-perps", "sai-website"]
  agent_skills: # related skills
    - sai-keeper-graphql
    - sai-db
    - sai-rest-api
---

# Sai Perps: Contract Query Playbook

Source of truth for query message shapes: `$HOME/ki/sai-website/webapp/pages/easy.tsx` (`QUERY_CONFIG`).
Full validated CLI notes: `$HOME/ki/boku/epics/epic-sai/26-02-10-sai-mainnet-query.md`.

## Mainnet addresses

| Contract | Address |
|----------|---------|
| **Perp** | `nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph` |
| **Oracle** | `nibi1xfwyfwtdame6645lgcs4xvf4u0hpsuvxrcelfwtztu0pv7n4l6hqw5a8gj` |
| Vault — Group 0 / USDC | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| Vault — Group 0 / stNIBI | `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej` |
| Vault — Group 1 / USDC | `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8` |
| Vault — Group 1 / stNIBI | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |

Source: `$HOME/ki/sai-website/webapp/config/env.ts` (`SaiContractsMainnet`).

Group 0 = main crypto markets (BTC, ETH, SOL, …). Group 1 = Real Estate (Coded Estate).

## Strongly Related Skills

- `sai-keeper-graphql`: Same domain (Sai exchange) except via GraphQL. Use when
you need indexed data on perp trades, SLP vaults, oracle prices, or any
information used in the end user application. Instead of live smart contract
queries.
- `sai-db`: Explains how Sai data ends up in Postgres. The sai-keeper repo
GraphQL reads from this DB. Use when you need schema details, migrations info, to debug indexer logic, or to design and edit queries over indexed data.
- `sai-rest-api`: Broad interface for aggregated stats, yield. For quick metric
inspection on different dates.

## CLI setup

```bash
PERP="nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph"
ORACLE="nibi1xfwyfwtdame6645lgcs4xvf4u0hpsuvxrcelfwtztu0pv7n4l6hqw5a8gj"
NODE="https://rpc.archive.nibiru.fi:443"
# helper alias
sai_perps_q() { nibid query wasm contract-state smart "$1" "$2" --node "$NODE" --output json; }
```

## Rules of thumb

- Query keys are **snake_case** (CosmWasm serde). `GetMarket` → `get_market`.
- Some args use **wrapped string indices**: `"index": "MarketIndex(0)"`, `"index": "GroupIndex(0)"`, `"group_index": "GroupIndex(1)"`, `"collateral_index": "TokenIndex(1)"`.
- Some args use **plain integers**: `collateral_index: 1`, `market_index: 0`, `group_index: 0`.
  - When both arg names are plain integers, use integers. When the field name takes a typed index (e.g. `get_vault_address`), use the wrapped string form.
  - Check `QUERY_CONFIG` in `easy.tsx` for the exact template for each query.
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

Full list: all markets belong to `GroupIndex(0)` except real-estate markets (2–12) which belong to `GroupIndex(1)`.

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

## Curated query reference

Query message JSON templates, organized by contract. Source: `easy.tsx` `QUERY_CONFIG`.

### Perp contract

```json
{"list_markets":{}}
{"list_groups":{}}
{"list_collaterals":{}}
{"get_market":{"index":"MarketIndex(0)"}}
{"get_group":{"index":"GroupIndex(0)"}}
{"get_collateral":{"index":1}}
{"get_fees":{"index":"FeeIndex(0)"}}
{"get_pair_custom_max_leverage":{"index":0}}
{"get_borrowing_pair":{"collateral_index":1,"market_index":0}}
{"get_borrowing_pair_oi":{"collateral_index":1,"market_index":0}}
{"get_borrowing_pair_group":{"collateral_index":1,"market_index":0}}
{"get_borrowing_group":{"collateral_index":1,"group_index":0}}
{"get_borrowing_group_oi":{"collateral_index":1,"group_index":0}}
{"get_vault_address":{"group_index":"GroupIndex(0)","collateral_index":"TokenIndex(1)"}}
{"get_trade":{"trader":"nibi1...","index":0}}
{"get_trades":{"trader":"nibi1..."}}
{"get_trade_info":{"trader":"nibi1...","index":0}}
{"get_trade_infos":{"trader":"nibi1..."}}
{"get_trade_data":{"trader":"nibi1...","index":0}}
{"get_trade_pnl":{"trader":"nibi1...","index":0}}
{"get_liquidation_price":{"trade_id":"UserTradeIndex(0)","trader":"nibi1...","include_borrowing_fees":true}}
{"get_perp_prices":{"market_index":"MarketIndex(1)","collateral_index":"TokenIndex(1)"}}
{"get_trader_fee_multiplier":{"trader":"nibi1..."}}
{"is_trader_stored":{"trader":"nibi1..."}}
{"get_fee_tiers":{}}
{"get_pending_gov_fees":{"index":0}}
{"get_oracle_address":{}}
{"get_trading_activated":{}}
{"get_oi_windows_settings":{}}
{"get_windows":{"windows_duration":3600,"market_index":0,"current_window_id":0}}
{"get_pair_depth":{"index":0}}
{"get_user_deposit":{"user":"nibi1...","collateral_index":1}}
{"list_user_deposits":{"user":"nibi1..."}}
```

### Oracle contract

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

### Vault contract

```json
{"tvl":{}}
{"available_assets":{}}
{"market_cap":{}}
{"current_epoch":{}}
{"get_current_epoch_start":{}}
{"config":{}}
{"vault_snapshot":{}}
{"get_revenue_info":{}}
{"collateralization_p":{}}
{"withdraw_epochs_timelock":{}}
{"get_vault_share_denom":{}}
{"get_collateral_denom":{}}
{"max_mint":{}}
{"max_deposit":{}}
{"max_redeem":{"depositor":"nibi1..."}}
{"max_withdraw":{"depositor":"nibi1..."}}
{"total_shares_being_withdrawn":{"depositor":"nibi1..."}}
{"user_withdraw_requests":{"user":"nibi1..."}}
{"get_locked_deposit":{"deposit_id":0}}
{"all_locked_deposits":{"start_after":null,"limit":null}}
{"user_locked_deposits":{"user":"nibi1..."}}
{"lock_discount_p":{"collat_p":"1.0","lock_duration":0}}
{"user_vault_state":{"user":"nibi1..."}}
```

## TypeScript (NibiruQuerier) alternative

```typescript
import { Mainnet, NibiruQuerier } from "@nibiruchain/nibijs"

const querier = await NibiruQuerier.connect(Mainnet().endptTm)

const result = await querier.wasmClient.queryContractSmart(
  "nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph",
  { get_borrowing_group_oi: { collateral_index: 1, group_index: 0 } },
)
```

Address constants live in `$HOME/ki/sai-website/webapp/config/env.ts` (`SaiContractsMainnet`).
