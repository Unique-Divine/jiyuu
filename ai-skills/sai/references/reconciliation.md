# Sai Reconciliation

Use this reference when a Sai operator question compares two or more truth
surfaces: on-chain contracts, Sai Keeper GraphQL, DexPal REST, the `sai-keeper`
Postgres database, or operator records such as spreadsheets and notes.

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
- Operator records, such as a referral spreadsheet or deal notes, hold
  human-maintained metadata: names, deal terms, share URLs, and labels. They
  are precedent, not live state.

## Reconciliation workflow

1. Name each layer involved in the mismatch. For example: contract state,
   GraphQL field `trades`, REST endpoint `GET /dexpal/v1/stats`, database table
   `stats_perp_oi_daily`, or a spreadsheet column.
2. Decide which layer owns the requested fact.
3. Check freshness before interpreting values. Use block heights, timestamps,
   daily buckets, `lastUpdatedBlock`, and DB `MAX(ts)` queries where available.
4. Normalize units before comparing. Contract, GraphQL, and DB integer amounts
   are often base units; REST may expose human or USD values.
5. Explain differences by source. Example: "contract state says X, GraphQL has
   indexed Y through block Z, REST aggregates table `stats_perp_oi_daily`, and
   the spreadsheet label is human metadata."

## Common mismatch patterns

- REST endpoint `GET /dexpal/v1/stats` can show zero open interest when table
  `stats_perp_oi_daily` lacks a current-day row, even while contract state and
  table `perp_trade` show open positions.
- REST endpoint `GET /dexpal/v1/stats` builds its 24-hour volume, trade, and
  user figures from daily date buckets, not a rolling wall-clock window. REST
  endpoint `GET /dexpal/v1/markets/details` computes a separate 24-hour view
  and can use a latest-block timestamp anchor. A trade missing from an
  aggregate has two possible causes: it falls in another day's bucket, or
  ingestion or the daily rollup is behind. Check which bucket the trade's
  timestamp belongs to and how fresh the aggregate is before naming one. See
  [24-hour time-window semantics](rest.md#24-hour-time-window-semantics).
- GraphQL fields expose indexed state, not necessarily the same moment as a live
  contract query.
- Referral spreadsheet rows can be stale or incomplete because they are operator
  metadata. Verify code ownership and discounts on-chain.
- SLP Vault smart queries can hide details that raw state explains, especially
  around `acc_pnl_per_token_used`, `current_max_supply`, daily resets, and reward
  distribution accounting.

## Relevant references

- Contract state and query payloads: [`contracts.md`](contracts.md)
- SLP Vault health and raw state: [`vaults.md`](vaults.md)
- GraphQL fields and subscriptions: [`graphql.md`](graphql.md)
- REST endpoint semantics: [`rest.md`](rest.md)
- DB tables and SQL snippets: private agent skill `sai-db`
- Referral checks: [`referrals.md`](referrals.md)
