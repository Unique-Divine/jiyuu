# Sai Reconciliation

Use this reference when a Sai operator question compares two or more truth
surfaces: on-chain contracts, Sai Keeper GraphQL, DexPal REST, the `sai-keeper`
Postgres database, the Referral Identity Directory sheet, or boku operator notes.

## Source-of-truth model

- On-chain Sai contract state owns live protocol facts: market and collateral
  config, OI limits, borrowing state, fee config, vault mappings, referral code
  ownership, user deposits, and trading-credit balances.
- Sai Keeper GraphQL owns indexed app/backend facts: trade history, referral
  redemptions, LP history, fee analytics, oracle prices as indexed by the app,
  and subscription payloads.
- DexPal REST owns public aggregate responses: stats, market details, metrics,
  yield, referral reports, and health checks.
- The `sai-keeper` Postgres database explains how GraphQL and REST facts are
  stored, refreshed, and aggregated.
- The Referral Identity Directory sheet owns human-maintained metadata: Telegram
  names, Notion CRM links, notes, deal terms, share URLs, and operator labels.
- Boku epics and READMEs are operator records and precedent, not live state.

## Reconciliation workflow

1. Name each layer involved in the mismatch. For example: contract state,
   GraphQL field `trades`, REST endpoint `GET /dexpal/v1/stats`, database table
   `stats_perp_oi_daily`, or sheet column `saiCode`.
2. Decide which layer owns the requested fact.
3. Check freshness before interpreting values. Use block heights, timestamps,
   daily buckets, `lastUpdatedBlock`, and DB `MAX(ts)` queries where available.
4. Normalize units before comparing. Contract, GraphQL, and DB integer amounts
   are often base units; REST may expose human or USD values.
5. Explain differences by source. Example: "contract state says X, GraphQL has
   indexed Y through block Z, REST aggregates table `stats_perp_oi_daily`, and
   the sheet label is human metadata."

## Common mismatch patterns

- REST endpoint `GET /dexpal/v1/stats` can show zero open interest when table
  `stats_perp_oi_daily` lacks a current-day row, even while contract state and
  table `perp_trade` show open positions.
- REST endpoint `GET /dexpal/v1/stats` uses a wall-clock 24h window, while REST
  endpoint `GET /dexpal/v1/markets/details` can use a latest-block timestamp
  anchor.
- GraphQL fields expose indexed state, not necessarily the same moment as a live
  contract query.
- Referral sheet rows can be stale or incomplete because they are operator
  metadata. Verify code ownership and discounts on-chain.
- SLP Vault smart queries can hide details that raw state explains, especially
  around `acc_pnl_per_token_used`, `current_max_supply`, daily resets, and reward
  distribution accounting.

## Relevant references

- Contract state and query payloads: [`sai-contracts.md`](sai-contracts.md)
- SLP Vault health and raw state: [`sai-vaults.md`](sai-vaults.md)
- GraphQL fields and subscriptions: [`sai-graphql.md`](sai-graphql.md)
- REST endpoint semantics: [`sai-rest.md`](sai-rest.md)
- DB tables and SQL snippets: [`sai-db.md`](sai-db.md)
- Referral checks: [`sai-referrals.md`](sai-referrals.md)
