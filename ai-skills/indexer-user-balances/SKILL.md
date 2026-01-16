---
name: indexer-user-balances
description: Queries the Nibiru indexer (Heart Monitor) for user account and balance data. Use when querying user balances by address, the user/users root fields, Token amounts, or HeartMonitor.user/users in the TS SDK.
---

# Nibiru Indexer (Heart Monitor) — User Balances

Use this skill when working with the **Nibiru indexer / Heart Monitor** GraphQL API for **user** and **users** queries and **Token** balances (account balances by address).

## Quick Facts

- **Indexer** = Heart Monitor = GraphQL API. **Endpoints** (path `/query`): Mainnet `https://hm-graphql.nibiru.fi/query`; testnet `https://hm-graphql.itn-2.nibiru.fi/query`. **Env** is the hostname segment for the network: mainnet has no env (host `hm-graphql.nibiru.fi`); testnet uses env `itn-2` (host `hm-graphql.itn-2.nibiru.fi`). The playground is at `/graphql` on the same host.
- **No container**: Unlike staking (`query { staking { ... } }`), **user** and **users** are **root-level** fields on `Query`. Use `user(where: { address: "nibi1..." })` and `users(...)` directly.
- **`where.address`**: Accepts **Bech32** (`nibi1...`) or **EVM hex** (`0x...`). The resolver normalizes to the same account; **`user.address` in the response is always Bech32**.
- **`balances` vs `all_balances`**: **`balances`** is the legacy `{ denom, amount }` list and is often **empty** for accounts whose funds are mostly **EVM / FunToken / enriched** balances. Prefer **`all_balances`**: each row is a **Balance** with **`amount`** (string, base units) and **`token_info`** (**TokenInfo**: symbol, name, decimals, `bank_denom`, `erc20_contract_address`, `type`, etc.). See [reference.md — User, Balance, TokenInfo](reference.md#4-graphql-schema-types-heart-monitor).
- **Amount strings**: **Token.amount** and **Balance.amount** are **String** scalars (unlike staking Ints). Parse for display using **`token_info.decimals`** when present.
- **Pagination**: Same as indexer-staking — default limit 1000, max 1000; use `limit` (and for list endpoints, offset where supported) to page. See [reference.md](reference.md#6-pagination--limits).
- **TS SDK**: Use `HeartMonitor` from `@nibiruchain/nibijs`; methods `user()` and `users()` for these queries.

## Canonical Usage

- **Single user by address**: `user(where: { address: "nibi1..." })` or `user(where: { address: "0x..." })` with a selection set that includes **`all_balances`** when you need real balances, for example:

```graphql
user(where: { address: "0x..." }) {
  address
  balances { denom amount }
  all_balances {
    amount
    token_info {
      type
      name
      symbol
      bank_denom
      erc20_contract_address
      decimals
      verified
      price
      logo
    }
  }
  created_block { block }
  is_blocked
}
```

- **List users**: `users(where: ..., order_by: UserOrder, order_desc: Boolean, limit: Int)` with the same **`all_balances`** / **`balances`** shape as needed. Filter by `UsersFilter` (optional `address`, `created_block_eq`, `created_block_lte`, `created_block_gte`); order by `UserOrder` (`address`, `created_block`).

## Rule of Thumb: Where to Query?

| Requirement | Preferred Source | Reason |
| :--- | :--- | :--- |
| Current account balances (by address) | **Indexer** | Fast, indexed, GraphQL support. |
| Strict consistency or balance type not in indexer | **Chain** | Use `nibid query bank balances <address>` or chain RPC. |
| Indexer lag acceptable | **Indexer** | Data may be a few seconds behind the chain. |

## Additional Resources

- **[Full Reference (reference.md)](reference.md)**:
  - [GraphQL root: no container](reference.md#3-graphql-root-no-container)
  - [GraphQL schema types (User, balances, all_balances, Token, Balance, TokenInfo)](reference.md#4-graphql-schema-types-heart-monitor)
  - [Filters and order](reference.md#5-filters-and-order)
  - [Pagination and limits](reference.md#6-pagination--limits)
  - [TS SDK (Heart Monitor)](reference.md#7-ts-sdk-heart-monitor)
  - [Gotchas](reference.md#8-gotchas)
  - [File reference (by repo)](reference.md#9-file-reference-by-repo)
