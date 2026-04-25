# Skill sai-db Conventions

This document explains common patterns and semantics used across the `sai-keeper` database tables.

## 1. Event Coordinates
Most historical tables (ending in `_history`) use a set of coordinates to uniquely identify an on-chain event:
- **block**: The block height.
- **tx_index**: The index of the transaction within the block.
- **event_index**: The index of the event within the transaction.

Together, `(block, tx_index, event_index)` provide a stable order and ensure uniqueness for indexed events.

## 2. Timestamps and Dates
- **timestamptz**: Used for real-world time (e.g., `block_ts` in the `block` table).
- **date**: Used in `stats_*` tables for daily buckets (e.g., `ts` in `stats_perp_by_user`).
- **block**: Often used as a proxy for time in `_history` tables.

## 3. "History" vs. "State" Tables
- **State Tables** (e.g., `perp_trade`, `lp_vault`): Store the *current* state of an entity. They are updated in place as new events arrive.
- **History Tables** (e.g., `perp_trade_history`, `lp_deposit_history`): Store an immutable log of changes. Each row represents a specific action or event.

## 4. Units and Decimals
- **Base Units (Int/Bigint)**: Fields like `collateral_amount`, `tvl`, and `available_assets` are stored in base units (e.g., 6 decimals for USDC, 18 for some EVM tokens).
- **USD Values (Float8/Numeric)**: Fields like `price_usd`, `volume_usd`, and `realized_pnl_usd` are human-readable floats or high-precision numerics already scaled to USD.
- **Percentages (Float8)**: Fields like `pnl_pct` or `spread_p` are stored as decimals (e.g., `0.01` for 1%).

## 5. Partitioning
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

## 6. Identifiers
- **IDs**: Many protocol entities use `bigint` IDs assigned by the smart contracts (e.g., `perp_market_id`, `trade_id`).
- **Addresses**: Trader and vault addresses are stored as `text` (e.g., Bech32 `nibi1...` or hex `0x...`).
