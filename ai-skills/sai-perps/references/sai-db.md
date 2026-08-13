# Sai database queries and schema

Use this reference for the `sai-keeper` Postgres schema, migrations, tables,
columns, SQL types, units, API surfacing, and aggregate freshness. It is a
read/query snapshot. For direct Tailscale access, primary/replica routing, or
write operations, use the private agent skill `sai-db` rather than treating this
reference as the live operator runbook.

## How to use this reference

1. **Look up tables and columns**: See [schema reference](#schema-reference) for a domain-by-domain breakdown of Oracle, Perp, LP, Referral, and Stats data.
2. **Understand data patterns**: See [database conventions](#database-conventions) for coordinates, units, and partitioning.
3. **Find where data surfaces**: See [API surfacing](#api-surfacing) for mappings between database tables and API endpoints.

### Direct database access (`psql`)

Use direct SQL when validating endpoint behavior, unit semantics, and aggregate freshness.

Before connecting to Sai Keeper Postgres, verify that the workstation is on the
company private network. The database host is reachable through Tailscale/VPN.

```bash
tailscale status
```

If Tailscale is not active or the expected private nodes are unavailable, stop
and report that direct database access is blocked by network connectivity.

For operator DB checks on the Tailscale network, prefer local `psql` database
names over repository runtime environment variables.

Credentials are expected to already work via the workstation's local Postgres
setup. **Do not read or print** password files such as `~/.pgpass`, and do not
search for credentials or dump secret environment values. If direct `psql`
connection fails, report the connectivity or database selection issue to the
user (for testnet vs mainnet, see private skill `sai-db` on ambient
`PGPASSWORD` override).

```bash
SAI_DB="sai_keeper" # or "sai_keeper_2" depending on time
```

- **Local default DB name**: `sai_keeper` (override as needed) OR `sai_keeper_2`
  if specified by user.
- **Preferred connect order**:
  - `psql -d sai_keeper_2 -X -P pager=off -c "SELECT current_database(), current_user;"`
  - `psql -d sai_keeper -X -P pager=off -c "SELECT current_database(), current_user;"`
- **Basic connect**: `psql -d sai_keeper`
- **Run one query inline**: `psql -d sai_keeper -c "SELECT 1 AS ok;"`
- **List available databases when ambient `psql` connects to the wrong database**:
  `psql -X -P pager=off -c "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname;"`

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

- **[sai-graphql.md](sai-graphql.md)**: Most tables are exposed via GraphQL queries and subscriptions.
- **[sai-rest.md](sai-rest.md)**: High-level metrics (volume, OI, TVL) are served via REST primarily from `stats_*` table queries (with `stats_cache` as legacy/auxiliary context).

## Quick start

Use [schema reference](#schema-reference) to see all available
tables with explanations for each column.


### Data source

The **sai-keeper** database schema is the source of truth for the Sai exchange's indexed data. The schema is defined by SQL migrations in the `sai-keeper` repository.

- **Schema Location**: `$HOME/ki/sai-keeper/models/migrations/`
- **Routines/Stored Procs**: `$HOME/ki/sai-keeper/models/routines/` (maintains stats/aggregates)

## High-value SQL snippets

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

## Known pitfalls and interpretation notes

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

## Playbook for user questions

- **"What fields are in the perp_trade table?"** -> Consult `schema.md#perp_trade`.
- **"How is volume calculated?"** -> Consult `conventions.md#units` and `surfacing.md#rest-api` (derived from `stats_perp_by_user`).
- **"What fields are in the stats_perp_by_user table?"** -> Consult `schema.md#stats_perp_by_user`. Includes volume, PnL, and trade counts per user/market/collateral.
- **"Where are liquidations stored?"** -> Liquidations are records in `perp_trade_history` with `trade_change_type = 'position_liquidated'`. See `schema.md#perp_trade_history`.


---

## Schema reference

This document provides a detailed breakdown of the tables and types in the
`sai-keeper` database.

### Common (`001_common.sql`)

#### Tables
- **schema_version**: Tracks the current migration version.
  - `version`: bigint (PK)
- **historical_sync**: Tracks blocks synced during historical indexing.
  - `id`: bigserial (PK)
  - `batch_start_block`: bigint
  - `batch_end_block`: bigint
  - `created_at`: timestamptz
- **exchange_state**: Current operating state of the exchange.
  - `state`: exchange_status (active, paused, close_only)
  - `last_updated_block`: bigint
- **exchange_config**: Protocol-wide configuration parameters.
  - `max_pnl`, `max_sl`, `liquidation_threshold`: float8
  - `fee_tiers`, `price_impact`: jsonb
  - `last_updated_block`: bigint
- **block**: Maps block numbers to timestamps.
  - `block`: bigint (PK)
  - `block_ts`: timestamptz
- **tx**: Maps transaction hashes to blocks and indexes.
  - `block`: bigint, `tx_index`: bigint (Composite PK)
  - `tx_hash`: text, `evm_hash`: text

#### Types
- **string_pair**: (key1 text, key2 text)
- **exchange_status**: `active`, `paused`, `close_only`

---

### Oracle (`002_oracle.sql`)

#### Tables
- **oracle_token**: Metadata for assets tracked by the oracle.
  - `id`: bigint (PK)
  - `base`: text, `permission_group`: bigint
- **oracle_token_info**: Visual/display metadata for tokens.
  - `oracle_token_id`: bigint (PK, FK)
  - `logo_url`: text, `symbol`: text, `trading_view_symbol`: text
- **oracle_token_price_source**: External sources for token prices.
  - `oracle_token_id`: bigint, `price_source`: text (Composite PK)
  - `symbol`: text
- **oracle_price**: Latest market price for each token.
  - `oracle_token_id`: bigint (PK)
  - `price_usd`: float8, `last_updated_block`: bigint
- **oracle_price_history**: Time-series of oracle prices.
  - `oracle_token_id`: bigint, `block`: bigint (Composite PK)
  - `price_usd`: float8
  - *Partitioned by block range.*

---

### Perpetuals (`003_perp.sql`)

#### Tables
- **perp_collateral**: Supported collateral assets.
  - `oracle_token_id`: bigint (PK), `is_active`: boolean
- **perp_market**: Trading pairs (e.g. BTC/USDC).
  - `id`: bigint (PK)
  - `base_token_id`, `quote_token_id`: bigint
  - `spread_p`, `max_leverage`: float8
  - `perp_market_group_id`, `perp_fee_id`: bigint
- **perp_market_group**: Groups of markets (e.g. Majors, Altcoins).
  - `id`: bigint (PK)
  - `name`: text, `min_leverage`, `max_leverage`: float8
- **perp_market_group_vault**: Maps market groups to specific collateral vaults.
  - `perp_market_group_id`, `collateral_id`: bigint (Composite PK)
  - `vault`: text (contract address)
- **perp_market_price**: Latest price for a perp market.
  - `perp_market_id`: bigint (PK)
  - `price`: float8, `last_updated_block`: bigint
- **perp_market_visibility**: Toggles market visibility in frontend.
  - `perp_market_id`: bigint (PK, FK), `visible`: boolean
- **perp_borrowing_group**: Borrowing rate metrics for a group and collateral.
  - `group_id`, `collateral_id`: bigint (Composite PK)
  - `acc_fees_long`, `acc_fees_short`: float8
  - `oi_long`, `oi_short`, `oi_max`: bigint
  - `fee_per_block`, `fee_exponent`: float8
  - `acc_last_updated_block`, `last_updated_block`, `last_updated_tx_index`,
    `last_updated_event_index`: bigint
- **perp_borrowing**: Borrowing rate metrics for a specific market and
  collateral.
  - Same fields as `perp_borrowing_group` plus `perp_market_id`,
    `borrowing_group_id`.
- **perp_trade**: Current state of open and pending trades.
  - `trader`: text, `id`: bigint (Composite PK)
  - `trade_type`: perp_trade_type, `perp_market_id`: bigint, `collateral_id`:
    bigint
  - `collateral_amount`, `open_collateral_amount`: bigint
  - `is_long`, `is_open`: boolean, `leverage`, `open_price`, `close_price`:
    float8
  - `tp`, `sl`: float8, `referral_code`: text
  - `open_block`, `close_block`: bigint, `last_updated_*`: bigint
- **perp_trade_history**: Event log of trade changes (opens, closes,
  liquidations).
  - `id`: bigserial (PK)
  - `trader`: text, `trade_id`: bigint, `trade_change_type`:
    perp_trade_change_type
  - `realized_pnl_pct`: float8, `block`, `tx_index`, `event_index`: bigint
- **perp_trade_state**: Derived trade metrics (real-time PnL, liq price).
  - `trader`: text, `id`: bigint (Composite PK)
  - `pnl_pct`, `pnl_collateral`, `borrowing_fee_*`, `closing_fee_*`,
    `pnl_collateral_after_fees`: float8
  - `remaining_collateral_after_fees`, `liquidation_price`: float8
- **perp_trade_trigger_history**: History of limit/stop order executions.
  - `id`: bigserial (PK)
  - `trader`: text, `trade_id`: bigint, `executor`: text
  - `trigger_type`: perp_trade_trigger_type
  - `trade_trigger_price`, `market_price`: float8
  - `trigger_execution_block`, `trigger_executed_block`: bigint, `is_success`:
    boolean, `error_msg`: text
- **perp_fee**: Fee configuration.
  - `id`: bigint (PK), `open_fee_p`, `close_fee_p`, `trigger_order_fee_p`:
    float8, `min_position_size_usd`: bigint
- **perp_fee_charged_history**: Detailed breakdown of fees paid per trade event.
  - `id`: bigserial (PK)
  - `trader`: text, `trade_id`: bigint, `fee_type`: perp_fee_type
  - `total_fee_charged`, `vault_fee`, `trigger_fee`, `gov_fee`, `net_gov_fee`,
    `referrer_allocation`: bigint
  - `fee_multiplier`: float8, `catalyst_address`: text, `collateral_left`,
    `bad_debt`: bigint
  - `open_fee_component`, `trigger_fee_component`, `triggerer_reward`,
    `final_closing_fee`: bigint
  - `block`, `tx_index`, `event_index`: bigint
- **perp_open_interest_history**: Time-series of OI per market/group.
  - `id`: bigint, `id_type`: perp_id_type, `collateral_id`: bigint, `block`,
    `tx_index`, `event_index` (Composite PK)
  - `oi_long`, `oi_short`: bigint
- **perp_pending_acc_fees_history**: Time-series of accumulated fees.
  - `id`: bigint, `id_type`: perp_id_type, `collateral_id`: bigint, `block`,
    `tx_index`, `event_index` (Composite PK)
  - `acc_fee_long`, `acc_fee_short`: float8
- **perp_balance_history**: History of contract balances.
  - `block`: bigint, `denom`: text (Composite PK), `balance`: bigint
- **perp_blacklist**: Blacklisted traders.
  - `trader`: text (PK), `blacklisted_ts`: timestamptz, `reason`: text

#### Types
- **perp_id_type**: `market`, `group`
- **perp_trade_type**: `trade`, `stop`, `limit`
- **perp_trade_change_type**: `position_opened`, `limit_order_created`,
  `stop_order_created`, `order_triggered`, `position_closed_tp`,
  `position_closed_sl`, `position_liquidated`, `position_closed_user`,
  `order_closed_user`, `tp_updated`, `sl_updated`, `leverage_updated`
- **perp_trade_trigger_type**: `limit_open`, `stop_open`, `tp_close`,
  `sl_close`, `liq_close`
- **perp_fee_type**: `opening`, `closing`

---

### LP (`004_lp.sql`)

#### Tables
- **lp_vault**: Current state of SLP vaults.
  - `address`: text (PK)
  - `shares_denom`, `shares_erc20`, `collateral_denom`, `collateral_erc20`:
    text
  - `collateral_id`, `available_assets`, `tvl`, `net_profit`, `rewards`,
    `closed_pnl`, `liabilities`, `current_epoch_positive_open_pnl`,
    `current_epoch`, `epoch_start`: bigint
  - `share_price`: float8
- **lp_vault_history**: Historical snapshot of vault metrics.
  - `vault`: text, `block`: bigint (Composite PK)
  - Same fields as `lp_vault`.
- **lp_deposit_history**: Log of deposit and withdrawal events.
  - `id`: bigserial (PK)
  - `action`: lp_action_type, `depositor`: text, `vault`: text
  - `amount`, `shares`: bigint, `block`, `tx_index`, `event_index`: bigint
- **lp_vault_assets_received_history**: External asset transfers to vaults
  (fees, etc).
  - `vault`: text, `block`: bigint, `tx_index`: bigint, `event_index`: bigint
    (Composite PK)
  - `sender`: text, `denom`: text, `amount`, `assets_less_deplete`: bigint
- **lp_vault_pnl_history**: History of PnL changes and distribution to LPs.
  - `vault`: text, `block`: bigint, `tx_index`: bigint, `event_index`: bigint
    (Composite PK)
  - `current_epoch`, `prev_positive_open_pnl`, `new_positive_open_pnl`,
    `current_epoch_positive_open_pnl`: bigint
  - `acc_pnl_per_token_used`: float8
- **lp_withdraw_request**: Pending user withdrawal requests.
  - `depositor`: text, `vault`: text, `unlock_epoch`: bigint (Composite PK)
  - `shares`: bigint, `auto_redeem`: boolean

#### Types
- **lp_action_type**: `deposit`, `create_withdraw_request`,
  `cancel_withdraw_request`, `redeem`

---

### Referral (`005_referral.sql`)

#### Tables
- **referral_code**: Registered referral codes.
  - `code`: text (PK), `referrer`: text, `description`: text, `created_block`:
    bigint, `is_active`: boolean
- **referral_code_creation_history**: Log of code creation.
  - `id`: bigserial (PK), `code`, `referrer`, `description`: text, `block`,
    `tx_index`, `event_index`: bigint
- **referral_redeem_history**: Log of users redeeming codes.
  - `id`: bigserial (PK), `trader`, `code`, `referrer`: text, `block`,
    `tx_index`, `event_index`: bigint
- **referral_claim_history**: History of referral commission claims.
  - `id`: bigserial (PK), `referrer`: text, `amount`: bigint, `denom`: text,
    `coin_breakdown`: jsonb, `chain`: text, `block`, `tx_index`, `event_index`:
    bigint

---

### Stats (`006_stats.sql`)

#### table `stats_block_ranges`
Maps time buckets to block ranges.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_block_ranges"
```
- `granularity` (stats_granularity): Bucket size (`1m`, `1h`, `1d`, etc); part
  of composite PK.
- `ts` (timestamptz): Bucket timestamp; part of composite PK.
- `start_block` (bigint): First chain block included in the bucket.
- `end_block` (bigint): Last chain block included in the bucket.

#### table `stats_oracle_price`
Aggregated oracle prices over time.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_oracle_price"
```
- `granularity` (stats_granularity): Aggregation interval; part of composite
  PK.
- `ts` (timestamptz): Bucket timestamp; part of composite PK.
- `oracle_token_id` (bigint): Oracle token ID; part of composite PK.
- `avg_price_usd` (float8): Average token price in USD for the bucket.
- `min_price_usd` (float8): Minimum token price in USD for the bucket.
- `max_price_usd` (float8): Maximum token price in USD for the bucket.
- `open_price_usd` (float8): First observed token price in USD for the bucket.
- `close_price_usd` (float8): Last observed token price in USD for the bucket.

#### table `stats_cache`
Singleton cache for global exchange metrics.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_cache"
```
- `id` (integer): Singleton row identifier (`CHECK (id = 1)`).
- `trading_volume_24h` (numeric): Rolling 24h trading volume in USD.
- `trading_volume_all_time` (numeric): Cumulative trading volume in USD.
- `total_trades_24h` (bigint): Rolling 24h trade count.
- `total_trades_all_time` (bigint): Cumulative trade count.
- `open_interest` (numeric): Current open interest in USD.
- `total_users_24h` (bigint): Distinct active users in last 24h.
- `total_users_all_time` (bigint): Cumulative distinct users.
- `total_open_positions` (bigint): Count of currently open positions.
- `tvl` (numeric): Total value locked in USD.
- `accrued_trading_fees_24h` (numeric): Rolling 24h accrued trading fees in
  USD.
- `accrued_trading_fees_all_time` (numeric): Cumulative accrued trading fees in
  USD.
- `last_updated` (timestamptz): Last cache refresh timestamp.

#### table `stats_perp_by_user`
User-level trading stats (daily).
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_perp_by_user"
```
- `ts` (date): Snapshot date.
- `user_address` (text): User wallet address.
- `market_id` (bigint): Perp market ID.
- `collateral_id` (bigint): Collateral token ID.
- `volume_usd_long` (numeric): Long-side volume in USD.
- `volume_usd_short` (numeric): Short-side volume in USD.
- `positive_pnl_usd` (numeric): Cumulative profits (positive PnL) in USD.
- `negative_pnl_usd` (numeric): Cumulative losses (negative PnL) in USD.
- `trades_count_long` (bigint): Total count of long trades.
- `trades_count_short` (bigint): Total count of short trades.
- `updated_at` (timestamptz): Timestamp of the last record refresh.

#### table `stats_perp_liq_daily`
Daily liquidation volume and counts per market.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_perp_liq_daily"
```
- `ts` (date): Snapshot date; part of composite PK.
- `market_id` (bigint): Perp market ID; part of composite PK.
- `collateral_id` (bigint): Collateral token ID; part of composite PK.
- `trades_count_long` (bigint): Number of liquidated long positions.
- `trades_count_short` (bigint): Number of liquidated short positions.
- `volume_long_usd` (numeric): Liquidated long notional volume in USD.
- `volume_short_usd` (numeric): Liquidated short notional volume in USD.
- `total_fee_charged_usd` (numeric): Total liquidation-related fees charged in
  USD.
- `updated_at` (timestamptz): Timestamp of the last record refresh.

#### table `stats_perp_oi_daily`
Daily Open Interest snapshots per market.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_perp_oi_daily"
```
- `ts` (date): Snapshot date; part of composite PK.
- `market_id` (bigint): Perp market ID; part of composite PK.
- `collateral_id` (bigint): Collateral token ID; part of composite PK.
- `oi_long_usd` (numeric): Long-side open interest in USD.
- `oi_short_usd` (numeric): Short-side open interest in USD.
- `updated_at` (timestamptz): Timestamp of the last record refresh.

#### table `stats_slp_deposit_by_user`
User LP stats (daily).
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_slp_deposit_by_user"
```
- `ts` (date): Snapshot date; part of composite PK.
- `user_address` (text): User wallet address; part of composite PK.
- `vault` (text): SLP vault address; part of composite PK.
- `amount_deposited_usd` (numeric): USD value deposited during this day.
- `amount_withdrawn_usd` (numeric): USD value withdrawn during this day.
- `cumulative_amount_deposited_usd` (numeric): Running cumulative deposited USD
  for user/vault.
- `cumulative_amount_withdrawn_usd` (numeric): Running cumulative withdrawn USD
  for user/vault.
- `updated_at` (timestamptz): Timestamp of the last record refresh.

#### table `stats_perp_sl_tp_daily`
Daily SL/TP execution stats.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_perp_sl_tp_daily"
```
- `ts` (date): Snapshot date; part of composite PK.
- `market_id` (bigint): Perp market ID; part of composite PK.
- `collateral_id` (bigint): Collateral token ID; part of composite PK.
- `sl_trades_count_long` (bigint): Number of long trades closed via stop-loss.
- `sl_trades_count_short` (bigint): Number of short trades closed via stop-loss.
- `sl_losses_prevented_long_usd` (numeric): Estimated long-side losses prevented
  by stop-loss in USD.
- `sl_losses_prevented_short_usd` (numeric): Estimated short-side losses
  prevented by stop-loss in USD.
- `tp_trades_count_long` (bigint): Number of long trades closed via take-profit.
- `tp_trades_count_short` (bigint): Number of short trades closed via
  take-profit.
- `tp_profit_realized_long_usd` (numeric): Realized long-side take-profit in
  USD.
- `tp_profit_realized_short_usd` (numeric): Realized short-side take-profit in
  USD.
- `updated_at` (timestamptz): Timestamp of the last record refresh.

#### table `stats_perp_fee_daily`
Daily fee breakdown per market.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_perp_fee_daily"
```
- `ts` (date): Snapshot date; part of composite PK.
- `market_id` (bigint): Perp market ID; part of composite PK.
- `collateral_id` (bigint): Collateral token ID; part of composite PK.
- `opening_fee_count` (integer): Count of opening-fee events.
- `opening_fee_total` (bigint): Total opening fees in base collateral units.
- `opening_fee_total_usd` (float8): Total opening fees in USD.
- `opening_vault_fee` (bigint): Opening-fee portion allocated to vaults (base
  units).
- `opening_vault_fee_usd` (float8): Opening-fee portion allocated to vaults
  (USD).
- `opening_trigger_fee` (bigint): Opening trigger-order fees (base units).
- `opening_trigger_fee_usd` (float8): Opening trigger-order fees (USD).
- `opening_gov_fee` (bigint): Opening-fee protocol/governance allocation (base
  units).
- `opening_gov_fee_usd` (float8): Opening-fee protocol/governance allocation
  (USD).
- `opening_referrer_fee` (bigint): Opening-fee referrer allocation (base units).
- `opening_referrer_fee_usd` (float8): Opening-fee referrer allocation (USD).
- `closing_fee_count` (integer): Count of closing-fee events.
- `closing_fee_total` (bigint): Total closing fees in base collateral units.
- `closing_fee_total_usd` (float8): Total closing fees in USD.
- `closing_vault_fee` (bigint): Closing-fee portion allocated to vaults (base
  units).
- `closing_vault_fee_usd` (float8): Closing-fee portion allocated to vaults
  (USD).
- `closing_trigger_fee` (bigint): Closing trigger-order fees (base units).
- `closing_trigger_fee_usd` (float8): Closing trigger-order fees (USD).
- `closing_gov_fee` (bigint): Closing-fee protocol/governance allocation (base
  units).
- `closing_gov_fee_usd` (float8): Closing-fee protocol/governance allocation
  (USD).
- `closing_referrer_fee` (bigint): Closing-fee referrer allocation (base units).
- `closing_referrer_fee_usd` (float8): Closing-fee referrer allocation (USD).
- `total_bad_debt` (bigint): Aggregate bad debt amount in base collateral units.
- `updated_at` (timestamptz): Timestamp of the last record refresh.

#### table `stats_referrals_daily`
Daily referrer earnings and volume.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_referrals_daily"
```
- `ts` (date): Snapshot date; part of composite PK.
- `referrer` (text): Referrer address; part of composite PK.
- `trades_count` (bigint): Number of referred trades for the day.
- `earnings_usd` (float8): Referrer earnings in USD for the day.
- `volume_usd` (float8): Referred trading volume in USD for the day.
- `updated_at` (timestamptz): Timestamp of the last record refresh.

#### table `stats_slp_vault`
Aggregated SLP vault metrics (APY, TVL, volume).
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_slp_vault"
```
- `ts` (timestamptz): Bucket timestamp; part of composite PK.
- `granularity` (stats_granularity): Bucket size; part of composite PK.
- `vault` (text): SLP vault address; part of composite PK.
- `share_price` (float8): Vault share price at snapshot time.
- `apy` (float8): Annual percentage yield estimate.
- `deposits` (float8): Deposited collateral amount in token units for the
  bucket.
- `deposits_usd` (float8): Deposited amount converted to USD.
- `withdrawals` (float8): Withdrawn collateral amount in token units for the
  bucket.
- `withdrawals_usd` (float8): Withdrawn amount converted to USD.
- `volume` (float8): Total vault activity volume in token units.
- `volume_usd` (float8): Total vault activity volume in USD.
- `users` (bigint): Count of unique users active in the bucket.
- `new_users` (bigint): Count of first-time users in the bucket.
- `assets_usd` (float8): Vault assets under management in USD.
- `closed_pnl` (float8): Closed PnL attributable to the vault in USD.
- `liabilities` (float8): Vault liabilities in USD.
- `updated_at` (timestamptz): Timestamp of the last record refresh.

#### table `stats_perp_trade_snapshots`
Snapshots of active trades for PnL tracking.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_perp_trade_snapshots"
```
- `granularity` (stats_granularity): Snapshot interval; part of composite PK.
- `ts` (timestamptz): Snapshot timestamp; part of composite PK.
- `trader` (text): Trader address; part of composite PK.
- `trade_id` (bigint): Trade ID scoped to trader; part of composite PK.
- `collateral_amount` (bigint): Trade collateral amount in base units.
- `leverage` (float8): Position leverage at snapshot time.
- `pending_pnl_usd` (float8): Unrealized (pending) PnL in USD at snapshot time.
- `realized_pnl_usd` (float8): Realized PnL in USD attributable in the interval.
- `opened_in_period` (boolean): True if position opened during this interval.
- `closed_in_period` (boolean): True if position closed during this interval.

#### table `stats_user_portfolio`
Time-series of user PnL and volume.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d stats_user_portfolio"
```
- `granularity` (stats_granularity): Aggregation interval; part of composite
  PK.
- `ts` (timestamptz): Bucket timestamp; part of composite PK.
- `user_address` (text): User wallet address; part of composite PK.
- `realized_pnl_usd` (float8): Realized PnL in USD in this bucket.
- `realized_pnl_usd_cumulative` (float8): Running cumulative realized PnL in
  USD.
- `pending_pnl_usd_cumulative` (float8): Running cumulative unrealized (pending)
  PnL in USD.
- `volume_usd` (float8): Trading volume in USD in this bucket.
- `volume_usd_cumulative` (float8): Running cumulative trading volume in USD.
- `trades_count` (bigint): Number of trades in this bucket.
- `trades_count_cumulative` (bigint): Running cumulative trade count.

#### table `leaderboard`
Global PnL and volume leaderboard.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d leaderboard"
```
- `trader` (text): Trader address (PK).
- `volume_usd` (numeric): Cumulative trading volume in USD.
- `realized_pnl_usd` (numeric): Cumulative realized PnL in USD.
- `collateral_used_usd` (numeric): Cumulative collateral usage in USD.
- `last_tracked_block` (bigint): Last block included in leaderboard refresh.

#### table `volume_leaderboard`
Volume-only leaderboard.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d volume_leaderboard"
```
- `trader` (text): Trader address (PK).
- `volume_usd` (numeric): Cumulative trading volume in USD for ranking.
- `last_tracked_block` (bigint): Last block included in refresh.
- `threshold_block` (bigint): Optional lower-bound block used for scoped volume
  windows.

#### table `statsig_last_tracked_block`
Tracks progress of stats refreshers.
```bash
psql -d sai_keeper_2 -X -P pager=off -c "\d statsig_last_tracked_block"
```
- `event_type` (text): Stats event/process identifier (PK).
- `last_tracked_block` (bigint): Latest processed block for that event type.

#### Types
- **stats_granularity**: `1s`, `1m`, `5m`, `15m`, `1h`, `4h`, `6h`, `12h`,
  `1d`, `1w`, `1mo`


---

## Database conventions

This document explains common patterns and semantics used across the `sai-keeper` database tables.

### Event coordinates
Most historical tables (ending in `_history`) use a set of coordinates to uniquely identify an on-chain event:
- **block**: The block height.
- **tx_index**: The index of the transaction within the block.
- **event_index**: The index of the event within the transaction.

Together, `(block, tx_index, event_index)` provide a stable order and ensure uniqueness for indexed events.

### Timestamps and dates
- **timestamptz**: Used for real-world time (e.g., `block_ts` in the `block` table).
- **date**: Used in `stats_*` tables for daily buckets (e.g., `ts` in `stats_perp_by_user`).
- **block**: Often used as a proxy for time in `_history` tables.

### History and state tables
- **State Tables** (e.g., `perp_trade`, `lp_vault`): Store the *current* state of an entity. They are updated in place as new events arrive.
- **History Tables** (e.g., `perp_trade_history`, `lp_deposit_history`): Store an immutable log of changes. Each row represents a specific action or event.

### Units and decimals
- **Base Units (Int/Bigint)**: Fields like `collateral_amount`, `tvl`, and `available_assets` are stored in base units (e.g., 6 decimals for USDC, 18 for some EVM tokens).
- **USD Values (Float8/Numeric)**: Fields like `price_usd`, `volume_usd`, and `realized_pnl_usd` are human-readable floats or high-precision numerics already scaled to USD.
- **Percentages (Float8)**: Fields like `pnl_pct` or `spread_p` are stored as decimals (e.g., `0.01` for 1%).

### Partitioning
The `oracle_price_history` table is partitioned by `block` range using the `pg_partman` extension. This ensures that queries over historical prices remain performant as the dataset grows. 

Migrations for partitioned tables usually include a call to `partman.create_parent`:
```sql
SELECT partman.create_parent(
   p_parent_table => 'public.oracle_price_history',
   p_control => 'block',
   p_type => 'native',
   p_interval => '100000',
   ...
);
```

### Identifiers
- **IDs**: Many protocol entities use `bigint` IDs assigned by the smart contracts (e.g., `perp_market_id`, `trade_id`).
- **Addresses**: Trader and vault addresses are stored as `text` (e.g., Bech32 `nibi1...` or hex `0x...`).


---

## API surfacing

This document maps database tables to their respective API exposures in GraphQL and REST.

### GraphQL surfacing
[`sai-graphql.md`](sai-graphql.md) covers API usage. Most DB tables map to a
GraphQL type, but query fields are grouped below the root domain objects
`perp`, `lp`, `oracle`, and `referral`.

| DB Table | GraphQL Type / Root Field | Domain |
|----------|---------------------------|--------|
| `perp_trade` | `PerpTrade` / `perp.trade`, `perp.trades` | Perp |
| `perp_trade_history` | `PerpTradeHistoryItem` / `perp.tradeHistory` | Perp |
| `perp_borrowing` | `PerpBorrowing` / `perp.borrowing`, `perp.borrowings` | Perp |
| `lp_vault` | `LpVault` / `lp.vaults` | LP |
| `lp_deposit_history` | `LpDepositHistoryItem` / `lp.depositHistory` | LP |
| `oracle_price` | `TokenPriceUsd` / `tokenPricesUsd` | Oracle |
| `referral_code` | `ReferralCode` / `referral.referralCodes` | Referral |

**Note**: For live updates (Subscriptions), GraphQL watchers monitor these tables and push updates when rows change or are added.

### REST API surfacing
[`sai-rest.md`](sai-rest.md) covers high-level metrics. In current code paths,
`/dexpal/v1/stats` and `/dexpal/v1/metrics` are derived primarily from
`stats_*` table queries in the API service layer.

| DB Table | REST Endpoint | Metric(s) |
|----------|---------------|-----------|
| `stats_perp_by_user` | `/dexpal/v1/stats`, `/dexpal/v1/metrics` (indirect) | 24h/all-time volume, trades, users |
| `stats_perp_oi_daily` | `/dexpal/v1/stats`, `/dexpal/v1/metrics` | Open interest (daily snapshot sum) |
| `stats_perp_fee_daily` | `/dexpal/v1/stats`, `/dexpal/v1/metrics` | 24h/all-time accrued trading fees |
| `lp_vault` + `stats_oracle_price` | `/dexpal/v1/stats`, `/dexpal/v1/metrics` | TVL (vault units converted to USD) |
| `stats_slp_vault` | `/dexpal/v1/yield` | Vault APY, TVL, and descriptions |
| `volume_leaderboard` | `/dexpal/v1/leaderboard` (if active) | Top traders by volume |
| `stats_cache` | Legacy/auxiliary | Global cached metrics (not primary path for current stats endpoint implementation) |

### Data refreshers
Aggregated tables in the `stats` domain are maintained by stored procedures in `models/routines/`. These procs run periodically (e.g., via `statsig`) to pull data from "State" and "History" tables into the "Stats" tables.

Key Procs:
- `proc_update_stats_perp_by_user`: Aggregates trades into daily user stats.
- `update_stats_perp_oi_daily`: Computes daily open interest snapshots in USD.
- `proc_update_stats_perp_fee_daily`: Aggregates fee history into daily USD fees.
- `proc_update_volume_leaderboard`: Maintains the leaderboard from trade history.
- `func_refresh_stats_cache`: Updates `stats_cache` (legacy/auxiliary path).

### See also
- [sai-graphql.md](sai-graphql.md)
- [sai-rest.md](sai-rest.md)
