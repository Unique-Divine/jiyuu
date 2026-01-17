---
name: sai-keeper-graphql
description: >-
  Query the Sai GraphQL API (sai-keeper) for perp trading, LP vaults, oracle
  prices, and fees on Nibiru. Use when working with Sai exchange data, perp trades,
  liquidity pools, oracle prices, or fee analytics.
metadata:
  tags: ["sai-perps", "sai-website", "sai-keeper", "indexer"]
  agent_skills: # related skills
    - sai-db
---

# Sai Keeper GraphQL

## Quick Facts

- **sai-keeper** = Sai exchange GraphQL API. Source schema lives in `$HOME/ki/sai-keeper/graphql/graph/*.graphqls`.
- **HTTP endpoint**: Root path is `/query` (e.g. `https://sai-keeper.nibiru.fi/query`).
- **Endpoint Discrepancy**: Public/proxy deployments may use `/graphql` for HTTP and WS. Always verify the actual deployment path.
- **Playground**: Root URLs `https://sai-keeper.nibiru.fi/` and `https://sai-keeper.testnet-2.nibiru.fi/`.
- **WebSocket**: Same host with path `/query`; e.g. `wss://sai-keeper.nibiru.fi/query`.

## Environments

| Environment | HTTP (GraphQL) | Playground | WebSocket |
|-------------|----------------|------------|-----------|
| Mainnet | `https://sai-keeper.nibiru.fi/query` | `https://sai-keeper.nibiru.fi/` | `wss://sai-keeper.nibiru.fi/query` |
| Testnet | `https://sai-keeper.testnet-2.nibiru.fi/query` | `https://sai-keeper.testnet-2.nibiru.fi/` | `wss://sai-keeper.testnet-2.nibiru.fi/query` |

## Core Concepts

- **Sai Protocol**: Decentralized perpetual futures exchange on Nibiru Chain.
- **Contract Stack**: 5 Wasm contracts: Perp Manager, LP Vault, Oracle, Fee Manager, and Governance.
- **Data Domains**:
  - `perp`: Perpetual trading positions, history, and borrowing rates.
  - `lp`: Liquidity pool metrics, user deposits, and withdrawal requests.
  - `oracle`: Real-time and historical token prices.
  - `fee`: Fee transactions, daily statistics, and protocol revenue analytics.
- **Key Terms**:
  - **Collateral**: Assets used as margin for trades.
  - **Liquidation**: Forced closure of positions with insufficient margin.
  - **Borrowing Fees**: Sai's mechanism for balancing markets (funding rate).
  - **Epoch**: Time period for LP operations (deposits/withdrawals).
  - **TVL**: Total Value Locked in a vault (available assets + open position assets).

## Amount and Type Semantics

- **Amounts**: Numeric values like `collateralAmount` or `tvl` are `Int` in base units (e.g., 6 decimals for USDC/stNIBI).
- **Percentages/Prices**: Use `Float` (e.g., `pnlPct`, `priceUsd`, `sharePrice`).
- **Time**: Represented by the `Time` scalar (RFC3339 format).
- **Token Decimals**: `Token` (from Oracle) does not expose `decimals`. Use `TokenInfo` (in `Balance` via `userBalances` subscription) to get decimal metadata.

## Root Types and API Reference

### Perp

- **trade(trader, id)**: Get a specific trade.
- **trades(where!)**: List trades for a trader. `PerpTradesFilter` requires `trader`.
- **tradeHistory**: Detailed event log of trade modifications (opens, closes, liquidations, order triggers).
- **borrowing(marketId, collateralId)**: Get detailed market metrics.
- **borrowings**: List available markets.
- **referralInfo(userAddress)**: Get referral redemption details for a user.
- **referralCodes(owner)**: List referral codes owned by an address.
- **referralRedemption(user)**: Check if a user has redeemed a code.

**PerpTradeChangeType**: `position_opened`, `position_closed_user`, `position_closed_tp`, `position_closed_sl`, `position_liquidated`, `limit_order_created`, `stop_order_created`, `order_triggered`, `order_closed_user`, `tp_updated`, `sl_updated`.

### LP (Liquidity Provision)

- **vaults**: Metrics for SLP vaults (single-collateral model).
- **deposits**: Active LP positions for a user.
- **depositHistory**: Historical deposit and withdrawal events.
- **withdrawRequests**: Pending withdrawal requests with `unlockEpoch`.
- **RevenueInfo**: Tracks `NetProfit`, `TraderLosses`, `ClosedPnl`, `Liabilities`, and `CurrentEpochPositiveOpenPnl`.

### Oracle

- **tokens**: List metadata for all supported assets.
- **token(id)**: Specific asset metadata.
- **tokenPricesUsd**: Current market prices. Always check `lastUpdatedBlock` for freshness (warn if > 60s old).

### Fee

- **feeTransactions**: Detailed breakdown of every fee paid.
- **feeDailyStats**: Aggregated daily data by collateral and trader.
- **feeAnalytics**: Enhanced statistics including `avgFeeMultiplier`.
- **traderFeeSummary** / **protocolFeeSummary**: Aggregated revenue over time periods.

## Filters and Pagination

Common parameters across most list fields:

- **where**: Filter object (e.g., `trader`, `perpMarketId`, `isOpen`, `depositor`, `vault`).
- **order_by**: Domain-specific sorting (e.g., `sequence`, `trade_id`, `id`, `name`).
- **order_desc**: Boolean for descending order.
- **limit** / **offset**: Standard pagination (default limits apply).

**Filter Types**:
- `StringFilter`: `{ eq, like }`
- `IntFilter`: `{ eq, gt, gte, lt, lte }`
- `TimeFilter`: `{ eq, gt, gte, lt, lte }`

## Subscriptions

Real-time updates via WebSocket (`wss://`).

| Subscription | Filter (`where`) | Notes |
|--------------|------------------|-------|
| `tokenPricesUsd` | optional `SubTokenPriceUsdFilter` | Price feed for all or specific token. |
| `perpTrades` | `SubPerpTradesFilter!` | Real-time position updates for a trader. |
| `perpTradeHistory` | `SubPerpTradeHistoryFilter!` | Trade event updates for a trader. |
| `perpBorrowing` | `marketId!, collateralId!` | Single market borrowing rate updates. |
| `perpBorrowings` | none | All market availability updates. |
| `lpVaults` | none | Live TVL and APY updates for all vaults. |
| `lpDeposits` | `SubLpDepositsFilter!` | User LP share balance updates. |
| `lpDepositHistory` | `SubLpDepositHistoryFilter!` | Real-time deposit/withdrawal events. |
| `lpWithdrawRequests` | `SubLpWithdrawRequestsFilter!` | Withdrawal status updates. |
| `userBalances` | `SubUserBalancesFilter!` | Live wallet balance updates for all assets. Includes `TokenInfo` with `decimals`. |

## REST API

When running `sai-keeper` with the `-api` flag:

- **GET /api/stats**: Exchange statistics (JSON). Includes `trading_volume_24h`, `total_trades_24h`, `open_interest`, `tvl`, and `accrued_trading_fees_24h`.
- **GET /health**: Service health status.
- **GET /**: API overview and available endpoints.

## Calculations and Formulas

- **Position Value**: `Collateral Amount * Leverage` (USD if collateral price is known).
- **Borrowing APR**: `feesPerHour * 24 * 365 * 100`.
- **Deposit Value**: `(shares * sharePrice) / 10^decimals`.
- **APY**: `(NetProfit / TVL) * (365 / epochDurationDays) * 100`.
- **Liquidation (Long)**: `Entry Price * (1 - 1/Leverage + maintenance_margin)`.
- **Liquidation (Short)**: `Entry Price * (1 + 1/Leverage - maintenance_margin)`.
- **Fee**: `Total Fee = Vault Fee + Gov Fee + Trigger Fee`.
- **Dynamic Fee**: `Actual Fee = Base Fee * feeMultiplier`.

## Related Repos and Sources

- **sai-keeper** (`$HOME/ki/sai-keeper`): GraphQL schema and backend implementation.
- **sai-docs** (`$HOME/ki/sai-docs`): 
  - API References: `dev/sai-keeper/api-*.md`
  - Core Concepts: `dev/sai-keeper/core-concepts.md`
  - Deployment/Setup: `dev/sai-keeper/README.md`
- **sai-perps** (`$HOME/ki/sai-perps`): Core protocol logic (Borrowing, Fees).
- **sai-website/webapp**: Web frontend; reference for query implementation.

## Additional Documentation

For exhaustive details on GraphQL enums, accounting variables, fee tiers, and entity relationships, refer to the **[reference.md](./reference.md)** file in this skill directory.
