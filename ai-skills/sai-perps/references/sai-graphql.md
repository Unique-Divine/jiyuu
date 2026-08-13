# Sai Keeper GraphQL queries

Use this reference for indexed, app-facing Sai data: perp positions and
history, SLP Vault history, Oracle prices, referral analytics, and live
subscriptions. For live protocol state or contract-owned values, use
[`sai-contracts.md`](sai-contracts.md) instead.

## Source and endpoints

The source of truth is the GraphQL schema in directory
`$HOME/ki/sai-keeper/graphql/graph/`. File
`$HOME/ki/sai-website/webapp/config/chain.ts` builds the deployed endpoint with
path `/query`.

| Network | HTTP | WebSocket |
| --- | --- | --- |
| Mainnet | `https://sai-keeper.nibiru.fi/query` | `wss://sai-keeper.nibiru.fi/query` |
| Testnet2 | `https://sai-keeper.testnet-2.nibiru.fi/query` | `wss://sai-keeper.testnet-2.nibiru.fi/query` |

The host root serves the GraphQL playground. Verify a public deployment before
assuming that its schema has caught up with the local Keeper commit.

## Query domains

The root `Query` type exposes domain objects: `oracle`, `perp`, `lp`,
`referral`, `appVersion`, `saiPoints`, and `explorer`. Scope fields under their
domain rather than querying them at the root.

| Domain | Key fields | Notes |
| --- | --- | --- |
| `perp` | `trade`, `trades`, `tradeHistory`, `borrowing`, `borrowings`, `statsUserPortfolio`, `traderFeeTierProgress` | Indexed trades, market borrowing state, and portfolio/fee-tier summaries. `trades(where:)` requires a trader address. |
| `lp` | `vaults`, `depositHistory`, `withdrawRequests`, `vaultStats`, `epochDurationDays` | Indexed vault and LP history; not a replacement for live vault accounting. |
| `oracle` | `tokens`, `token`, `tokenPricesUsd` | App-indexed token metadata and prices. Use `lastUpdatedBlock` to assess freshness. |
| `referral` | `referralCodes`, `referralTrades`, `referralRedemption`, `referralRedemptionTrader`, `referralClaims`, `referralStats` | Historical referral data keyed by referrer or trader. |

There is no GraphQL `fee` root domain or fields such as `feeTransactions` in
the current schema. For fee details, use Perp trade history fields such as
`openingFeeUsd` and `closingFeeUsd`, contract fee queries, or database
references as appropriate.

## Filters, order, and units

- Perp query field `trades` uses `PerpTradesFilter`, whose required `trader`
  field can be narrowed by `perpMarketId`, `perpCollateralId`, and `isOpen`.
- Perp query field `tradeHistory` uses `PerpTradeHistoryFilter`, whose required
  `trader` field returns the trader's indexed mutations.
- Lists accept their schema-defined `order_by`, `order_desc`, `limit`, and
  `offset` arguments. Do not assume that an order enum applies to another
  domain.
- Many contract-sized values are GraphQL `Int` base units, including Perp trade
  collateral and open interest. Exceptions are deliberate: table `Balance`
  exposes `amount` as a string, and USD/price/percentage fields commonly use
  GraphQL `Float`.
- GraphQL type `Token` does not expose decimal metadata. Table `Balance` includes
  `token_info.decimals` when a wallet or Sai balance subscription is relevant.

## Subscriptions

The current `Subscription` type provides exactly these fields:

| Field | Required filter or arguments |
| --- | --- |
| `tokenPricesUsd` | Optional `SubTokenPriceUsdFilter` (`tokenId`) |
| `perpTrades` | `SubPerpTradesFilter!` (`trader`) |
| `perpTradeHistory` | `SubPerpTradeHistoryFilter!` (`trader`) |
| `perpBorrowings` | None |
| `perpBorrowing` | `marketId`, `collateralId` |
| `lpVaults` | None |
| `lpDepositHistory` | `SubLpDepositHistoryFilter!` (`depositor`, `vault`) |
| `lpWithdrawRequests` | `SubLpWithdrawRequestsFilter!` (`depositor`, `vault`) |
| `userBalances` | `SubUserBalancesFilter!` (`user`) |
| `userInfo` | `SubUserFilter!` (`user`) |

No `lpDeposits` subscription exists in the current schema. For a user’s vault
state, query the LP history/request fields or read live contract state according
to the fact being requested.

## Example queries

```graphql
query OpenTrades($trader: String!) {
  perp {
    trades(where: { trader: $trader, isOpen: true }) {
      id
      collateralAmount
      leverage
      isLong
      state { positionValue liquidationPrice }
    }
  }
}
```

```graphql
query ReferralHistory($referrer: String!) {
  referral {
    referralCodes(referrer: $referrer) { code active uniqueTraders volumeUSD }
    referralRedemption(referrer: $referrer) { trader code active block { block block_ts } }
  }
}
```

## Related references

- [`sai-contracts.md`](sai-contracts.md): live Perp, Oracle, and vault queries.
- [`sai-rest.md`](sai-rest.md): public aggregates rather than entity history.
- [`sai-db.md`](sai-db.md): table ownership, aggregation, and freshness.
