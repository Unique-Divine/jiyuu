---
name: sai-db
description: >-
  Comprehensive reference for the sai-keeper Postgres database schema,
  migrations, tables, columns, and SQL types. Use when the user asks about the
  database structure, specific tables, field meanings, or how on-chain data is
  stored in the Sai exchange.
metadata:
  tags: ["sai-perps", "sai-keeper", "indexer"]
  agent_skills: # related skills
    - sai-keeper-graphql
    - sai-rest-api
---

# Sai DB Reference

## How to use this skill

1.  **Look up tables/columns**: See [schema.md](schema.md) for a domain-by-domain breakdown (Oracle, Perp, LP, Referral, Stats).
2.  **Understand data patterns**: See [references/conventions.md](references/conventions.md) for common coordinates (block, tx, event), units, and partitioning.
3.  **Find where data surfaces**: See [references/api-surfacing.md](references/api-surfacing.md) for the mapping between DB tables and API endpoints.

### Direct Database Access (psql)

Use direct SQL when validating endpoint behavior, unit semantics, and aggregate freshness.

```bash
SAI_DB="sai_keeper" # or "sai_keeper_2" depending on time
```

- **Local default DB name**: `sai_keeper` (override as needed) OR `sai_keeper_2`
  if specified by user.
- **Basic connect**: `psql -d sai_keeper`
- **Explicit connect**: `psql -U <user> -h <host> -p 5432 -d sai_keeper`
- **Run one query inline**: `psql -d sai_keeper -c "SELECT 1 AS ok;"`

Useful flags:

- `-X`: do not read `~/.psqlrc` (reproducible output)
- `-P pager=off`: disable pager for long results
- `-A -t -F ','`: script-friendly unaligned output
- `-c "<sql>"`: execute a SQL command non-interactively

Suggested debugging workflow:

1. Verify row freshness first (`MAX(ts)`, `COUNT(*) WHERE ts=current_date`).
2. Compare API metric source table to a live recompute query.
3. Break totals by `collateral_id`/`market_id` to locate outliers quickly.

## Connections to APIs

This database powers two primary APIs. For usage instructions and query patterns, see their respective skills:

- **[sai-keeper-graphql]($HOME/.cursor/skills/sai-keeper-graphql/SKILL.md)**: Most tables are exposed via GraphQL queries and subscriptions.
- **[sai-rest-api]($HOME/.cursor/skills/sai-rest-api/SKILL.md)**: High-level metrics (volume, OI, TVL) are served via REST primarily from `stats_*` table queries (with `stats_cache` as legacy/auxiliary context).

## Quick Start

Use [schema.md](schema.md) to see all available tables with explanations for each column.


### Data Source

The **sai-keeper** database schema is the source of truth for the Sai exchange's indexed data. The schema is defined by SQL migrations in the `sai-keeper` repository.

- **Schema Location**: `$HOME/ki/sai-keeper/models/migrations/`
- **Routines/Stored Procs**: `$HOME/ki/sai-keeper/models/routines/` (maintains stats/aggregates)

## High-value SQL Snippets

### 1) Open positions vs current-day OI snapshot

```sql
SELECT
  (SELECT COUNT(*) FROM perp_trade WHERE is_open = true AND trade_type = 'trade') AS open_positions,
  (SELECT COALESCE(SUM(oi_long_usd + oi_short_usd), 0) FROM stats_perp_oi_daily WHERE ts = current_date) AS oi_today_usd;
```

### 2) Check daily OI freshness

```sql
SELECT
  current_date AS today,
  MAX(ts) AS latest_oi_day,
  COUNT(*) FILTER (WHERE ts = current_date) AS rows_for_today
FROM stats_perp_oi_daily;
```

### 3) Compare "today only" vs latest available OI

```sql
SELECT
  COALESCE((
    SELECT SUM(oi_long_usd + oi_short_usd)
    FROM stats_perp_oi_daily
    WHERE ts = current_date
  ), 0) AS oi_today_usd,
  COALESCE((
    SELECT SUM(oi_long_usd + oi_short_usd)
    FROM stats_perp_oi_daily
    WHERE ts = (SELECT MAX(ts) FROM stats_perp_oi_daily WHERE ts <= current_date)
  ), 0) AS oi_latest_available_usd;
```

### 4) OI by collateral on latest day

```sql
WITH d AS (
  SELECT MAX(ts) AS ts
  FROM stats_perp_oi_daily
  WHERE ts <= current_date
)
SELECT
  s.ts,
  s.collateral_id,
  ot.base AS collateral_symbol,
  SUM(s.oi_long_usd + s.oi_short_usd) AS oi_usd
FROM stats_perp_oi_daily s
JOIN d ON s.ts = d.ts
LEFT JOIN oracle_token ot ON ot.id = s.collateral_id
GROUP BY s.ts, s.collateral_id, ot.base
ORDER BY oi_usd DESC;
```

### 5) Top users by cumulative volume

```sql
SELECT
  user_address,
  SUM(volume_usd_long + volume_usd_short) AS cumulative_volume_usd
FROM stats_perp_by_user
GROUP BY user_address
ORDER BY cumulative_volume_usd DESC
LIMIT 50;
```

### 6) Top users by 30-day volume

```sql
SELECT
  user_address,
  SUM(volume_usd_long + volume_usd_short) AS volume_30d_usd
FROM stats_perp_by_user
WHERE ts >= current_date - INTERVAL '30 days'
GROUP BY user_address
ORDER BY volume_30d_usd DESC
LIMIT 50;
```

### 7) Get average price of Oracle token for a specific day
Should always be used instead of tabls `oracle_price_history` which is too heavy
```sql
SELECT
    sop.oracle_token_id,
    sop.avg_price_usd
FROM stats_oracle_price sop
WHERE
    sop.ts <= '2026-04-01'
    AND oracle_token_id = 1
LIMIT 1;
```

### 8) Trades information
Base query for building various trade stats.

```sql
SELECT
    pt.trader,
    pt.id AS trade_id,
    pt.is_long,
    pt.is_open,
    leverage,
    ot.base as market_token,
    otc.base as collateral_token,
    pt.collateral_amount * coll_price_open.avg_price_usd / 1e6 AS collateral_amount_usd,
    pt.collateral_amount * pt.leverage * coll_price_open.avg_price_usd / 1e6 AS size_usd,
    pt.open_price,
    pt.close_price,
    pth.realized_pnl_pct,
    pth.realized_pnl_pct * pt.collateral_amount * coll_price_close.avg_price_usd / 1e6 AS realized_pnl_usd,
    pt.open_block,
    bop.block_ts as open_ts,
    pt.close_block,
    bcl.block_ts as close_ts,
    pt.referral_code
FROM perp_trade pt
LEFT JOIN perp_trade_history pth -- to track realized pnl
    ON pth.trader = pt.trader
           AND pth.trade_id = pt.id
           AND pth.trade_change_type IN (
             'position_closed_tp',
             'position_closed_sl',
             'position_closed_user',
             'position_liquidated'
          )
JOIN oracle_token otc on otc.id = pt.collateral_id
JOIN perp_market pm on pm.id = pt.perp_market_id
JOIN oracle_token ot on ot.id = pm.base_token_id
JOIN block bop ON bop.block = pt.open_block
JOIN block bcl ON bcl.block = pt.close_block
LEFT JOIN LATERAL ( -- get collateral price at trade open
    SELECT
        sop.oracle_token_id,
        sop.avg_price_usd
    FROM stats_oracle_price sop
    WHERE
        sop.oracle_token_id = pt.collateral_id
        AND sop.ts <= bop.block_ts
    LIMIT 1
) coll_price_open ON TRUE
LEFT JOIN LATERAL ( -- get collateral price at trade close
    SELECT
        sop.oracle_token_id,
        sop.avg_price_usd
    FROM stats_oracle_price sop
    WHERE
        sop.oracle_token_id = pt.collateral_id
        AND sop.ts <= bcl.block_ts
    LIMIT 1
) coll_price_close ON TRUE
ORDER BY pt.close_block DESC, pt.open_block DESC;
```

### 9) Perp markets detailed info
```sql
SELECT
    pm.id,
    type,
    symbol,
    description,
    visible,
    is_open,
    LEAST(pm.max_leverage, pmg.max_leverage) AS max_leverage,
    pf.open_fee_p,
    pf.close_fee_p,
    ts.name AS trading_schedule_name
FROM perp_market pm
JOIN perp_market_group pmg ON pmg.id = pm.perp_market_group_id
JOIN perp_market_visibility pm_vis ON pm_vis.perp_market_id = pm.id
JOIN perp_fee pf ON pm.perp_fee_id = pf.id
JOIN oracle_token_info oti ON oti.oracle_token_id = pm.base_token_id
LEFT JOIN trading_schedule ts ON ts.id = oti.trading_schedule_id
ORDER BY type, pm.id
```

## Known Pitfalls / Interpretation Notes

- `open_positions` and `open_interest` can diverge:
  - `open_positions` is live from `perp_trade`
  - `open_interest` in REST stats comes from daily snapshots (`stats_perp_oi_daily`)
  - If current day rows are missing, `open_interest` can read as `0` while open positions are non-zero.
- Unit semantics:
  - Base units are `bigint` (`collateral_amount`, `tvl`, etc).
  - USD metrics require conversion: `base_units / 1e6 * price_usd` (this codebase currently assumes 6 decimals in several routines).
- OI spikes can come from snapshot inclusion semantics:
  - Daily OI procedure logic can include positions opened and closed on the same day.
  - That inflates daily OI vs strict end-of-day "still open" interpretation.
- `perp_borrowing` OI and `stats_perp_oi_daily` OI are related but not guaranteed identical at any instant; they are populated by different paths and timings.

## Playbook for User Questions

- **"What fields are in the perp_trade table?"** -> Consult `schema.md#perp_trade`.
- **"How is volume calculated?"** -> Consult `conventions.md#units` and `surfacing.md#rest-api` (derived from `stats_perp_by_user`).
- **"What fields are in the stats_perp_by_user table?"** -> Consult `schema.md#stats_perp_by_user`. Includes volume, PnL, and trade counts per user/market/collateral.
- **"Where are liquidations stored?"** -> Liquidations are records in `perp_trade_history` with `trade_change_type = 'position_liquidated'`. See `schema.md#perp_trade_history`.
