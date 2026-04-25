# Skill sai-db: API Surfacing (GraphQL & REST)

This document maps database tables to their respective API exposures in GraphQL and REST.

## 1. GraphQL Surfacing
The **sai-keeper-graphql** skill covers the API usage. Most DB tables map directly to GraphQL types.

| DB Table | GraphQL Type / Root Field | Domain |
|----------|---------------------------|--------|
| `perp_trade` | `Trade` / `trade`, `trades` | Perp |
| `perp_trade_history` | `TradeHistory` / `tradeHistory` | Perp |
| `perp_borrowing` | `Borrowing` / `borrowing`, `borrowings` | Perp |
| `lp_vault` | `Vault` / `vaults` | LP |
| `lp_deposit_history` | `DepositHistory` / `depositHistory` | LP |
| `oracle_price` | `TokenPriceUsd` / `tokenPricesUsd` | Oracle |
| `perp_fee_charged_history` | `FeeTransaction` / `feeTransactions` | Fee |
| `referral_code` | `ReferralCode` / `referralCodes` | Referral |

**Note**: For live updates (Subscriptions), GraphQL watchers monitor these tables and push updates when rows change or are added.

## 2. REST API Surfacing
The **sai-rest-api** skill covers high-level metrics. In current code paths, `/dexpal/v1/stats` and `/dexpal/v1/metrics` are derived primarily from `stats_*` table queries in the API service layer.

| DB Table | REST Endpoint | Metric(s) |
|----------|---------------|-----------|
| `stats_perp_by_user` | `/dexpal/v1/stats`, `/dexpal/v1/metrics` (indirect) | 24h/all-time volume, trades, users |
| `stats_perp_oi_daily` | `/dexpal/v1/stats`, `/dexpal/v1/metrics` | Open interest (daily snapshot sum) |
| `stats_perp_fee_daily` | `/dexpal/v1/stats`, `/dexpal/v1/metrics` | 24h/all-time accrued trading fees |
| `lp_vault` + `stats_oracle_price` | `/dexpal/v1/stats`, `/dexpal/v1/metrics` | TVL (vault units converted to USD) |
| `stats_slp_vault` | `/dexpal/v1/yield` | Vault APY, TVL, and descriptions |
| `volume_leaderboard` | `/dexpal/v1/leaderboard` (if active) | Top traders by volume |
| `stats_cache` | Legacy/auxiliary | Global cached metrics (not primary path for current stats endpoint implementation) |

## 3. Data Refreshers
Aggregated tables in the `stats` domain are maintained by stored procedures in `models/routines/`. These procs run periodically (e.g., via `statsig`) to pull data from "State" and "History" tables into the "Stats" tables.

Key Procs:
- `proc_update_stats_perp_by_user`: Aggregates trades into daily user stats.
- `update_stats_perp_oi_daily`: Computes daily open interest snapshots in USD.
- `proc_update_stats_perp_fee_daily`: Aggregates fee history into daily USD fees.
- `proc_update_volume_leaderboard`: Maintains the leaderboard from trade history.
- `func_refresh_stats_cache`: Updates `stats_cache` (legacy/auxiliary path).

## See Also
- [sai-keeper-graphql]($HOME/.cursor/skills/sai-keeper-graphql/SKILL.md)
- [sai-rest-api]($HOME/.cursor/skills/sai-rest-api/SKILL.md)
