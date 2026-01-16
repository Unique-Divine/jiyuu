# Nibiru Indexer (Heart Monitor) — User Balances Schema Reference

This document describes the Nibiru indexer (Heart Monitor) and the **user** / **users** / **Token** portion of its GraphQL schema: what the indexer is, how the TypeScript SDK exposes it, and the types and fields available for user balance queries.

- [1. Repo and indexer context](#1-repo-and-indexer-context)
- [2. What the indexer is and how it's exposed](#2-what-the-indexer-is-and-how-its-exposed)
- [3. GraphQL root: no container](#3-graphql-root-no-container)
- [4. GraphQL schema types (Heart Monitor)](#4-graphql-schema-types-heart-monitor)
- [5. Filters and order](#5-filters-and-order)
- [6. Pagination and limits](#6-pagination--limits)
- [7. TS SDK (Heart Monitor)](#7-ts-sdk-heart-monitor)
- [8. Gotchas](#8-gotchas)
- [9. File reference (by repo)](#9-file-reference-by-repo)

---

## 1. Repo and indexer context

For repo locations and cross-repo routing (nibi-go-hm, nibi-ts-sdk, nibi-chain, etc.), see the **repo-map** skill. The indexer schema lives in **nibi-go-hm**; the TypeScript client (HeartMonitor, generated types) lives in **nibi-ts-sdk**.

---

## 2. What the indexer is and how it's exposed

- **Heart Monitor** (or "hm") is the server that indexes the blockchain and exposes that data over **GraphQL**.
- **Implementation**: The server is implemented in the `nibi-go-hm` repo.
- **Endpoint**: Clients send POST requests to the GraphQL endpoint at path `/query`. **Production URLs**: Mainnet `https://hm-graphql.nibiru.fi/query`; testnet `https://hm-graphql.itn-2.nibiru.fi/query`. **Env** is the hostname segment that identifies the network: omit it for mainnet; use `itn-2` for testnet. Generic form: `https://hm-graphql.<env>.nibiru.fi/query` (for mainnet, use `hm-graphql.nibiru.fi` with no `<env>.` segment). The playground is at `/graphql` on the same host.
- **Scope**: User balances (and the `user` / `users` root fields) are one part of the API. The same API serves staking, governance, oracle, inflation, IBC, wasm, and more.

---

## 3. GraphQL root: no container

Unlike staking, which uses a container field `query { staking { ... } }`, **user** and **users** are **root-level** fields on `Query`. There is no `user: User!` container type.

Root signatures (from the schema):

```graphql
type Query {
  user(where: UserFilter!): User
  users(
    where: UsersFilter
    order_by: UserOrder
    order_desc: Boolean
    limit: Int
  ): [User!]!
  # ... other root fields (staking, governance, oracle, etc.)
}
```

- **user**: Takes a required **UserFilter** (address). Returns a single **User** (nullable if no such user).
- **users**: Takes optional **UsersFilter**, **UserOrder**, **order_desc**, and **limit**. Returns a non-null list of **User**.

---

## 4. GraphQL schema types (Heart Monitor)

The following types are defined in the schema and used by `user` / `users` and by **User**. Only these types are documented here.

### User

Defined in `user.graphqls`. Represents an account and its indexed balances.

```graphql
type User {
  address: String!
  balances: [Token]!
  all_balances: [Balance]!
  created_block: Block!
  is_blocked: Boolean
}
```

- **address**: Bech32 account address returned by the API (e.g. `nibi1...`), even when the lookup used an EVM `0x` address in **UserFilter**.
- **balances**: Legacy list of **Token** rows (`denom` + `amount`). Often **empty** on mainnet for wallets that hold assets surfaced primarily through **all_balances** (e.g. ERC20 balances the indexer attaches **TokenInfo** to). Still useful when populated.
- **all_balances**: Preferred for a **full balance view**. Each element is **Balance** (`amount` + **token_info**). Use this when **`balances`** looks empty but the account should have funds.
- **created_block**: The block at which the indexer first saw this account. Type is **Block**.
- **is_blocked**: Optional. Whether the account is currently blocked in the indexer.

### Balance

Used by **User.all_balances**.

```graphql
type Balance {
  amount: String!
  token_info: TokenInfo!
}
```

- **amount**: Balance in **base units** as a string (e.g. `"200000000"` for 200 USDC when **token_info.decimals** is `6`).
- **token_info**: Metadata for the asset (bank denom, ERC20 address, symbol, decimals, etc.). See **TokenInfo** below.

### TokenInfo

Used by **Balance.token_info**. Field names differ from **Token** (there is no `denom` on **TokenInfo**; use **bank_denom**).

Typical fields (see live schema in GraphQL introspection or `nibi-go-hm`):

- **type**: Asset class (e.g. `erc20`).
- **name**, **symbol**: Human-readable labels.
- **bank_denom**: Cosmos bank denomination when applicable; may be **empty** for pure ERC20-only representations.
- **erc20_contract_address**: EVM contract when **type** is ERC20-related.
- **decimals**: Use with **amount** for display.
- **verified**, **price**, **logo**: Optional metadata / pricing / branding.

Example selection for tooling:

```graphql
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
```

### Token

Defined in `common.graphqls`. Used by **User.balances** and elsewhere in the API.

```graphql
type Token {
  denom: String!
  amount: String!
}
```

- **denom**: Token denomination (e.g. `unibi`, or Token Factory denoms).
- **amount**: Balance amount as a **String**. Unlike staking delegation amounts (Int in base units), Token amounts are strings; parse for display or arithmetic as needed.

### Block

Defined in `block.graphqls`. Used by **User.created_block**.

```graphql
type Block {
  block: Int!
  block_ts: Int!
  num_txs: Int!
  block_duration: Float!
}
```

- **block**: Block height.
- **block_ts**: Unix timestamp of the block.
- **num_txs**: Number of transactions in the block.
- **block_duration**: Time taken to produce the block (float).

### UserFilter

Defined in `user.graphqls`. Required argument for the **user** root query.

```graphql
input UserFilter {
  address: String!
}
```

- **address**: Required. **Bech32** (`nibi1...`) or **EVM hex** (`0x...`). The server resolves both to the same account; the **User.address** field in the response is Bech32.

### UsersFilter

Defined in `user.graphqls`. Optional argument for the **users** root query.

```graphql
input UsersFilter {
  address: String
  created_block_eq: Int
  created_block_lte: Int
  created_block_gte: Int
}
```

- **address**: Optional. Filter by account address (exact match, if supported by the resolver).
- **created_block_eq / created_block_lte / created_block_gte**: Optional. Filter by the block height at which the user was first seen.

### UserOrder

Defined in `user.graphqls`. Sort key for **users** (with **order_desc**).

```graphql
enum UserOrder {
  address
  created_block
}
```

Use with **users** as `order_by: UserOrder` and `order_desc: Boolean` to control sort order.

---

## 5. Filters and order

| Input / enum | Used by | Summary |
| --- | --- | --- |
| **UserFilter** | `user(where: UserFilter!)` | **address** (required). |
| **UsersFilter** | `users(where: UsersFilter)` | **address** (optional), **created_block_eq**, **created_block_lte**, **created_block_gte** (optional Int). |
| **UserOrder** | `users(order_by: UserOrder, order_desc: Boolean)` | Enum: **address**, **created_block**. |

---

## 6. Pagination and limits

Pagination follows the same server rules as the rest of the indexer (e.g. staking): see `nibi-go-hm/graphql/graph/db/utils.go` for `DEFAULT_LIMIT` and `MAX_LIMIT`.

- **Default limit**: 1000 (when not specified).
- **Max limit**: 1000 (values above 1000 are clamped).
- **users**: The **users** query accepts **limit** only (no **offset** in the schema). Use **limit** to cap the number of users returned; for larger result sets, combine with **UsersFilter** (e.g. **created_block_gte** / **created_block_lte**) to page by block range if the resolver supports it, or rely on limit-only fetching.

---

## 7. TS SDK (Heart Monitor)

The TypeScript SDK (`@nibiruchain/nibijs` from the `nibi-ts-sdk` repo) provides a typed client for the Heart Monitor API.

### Import and construction

```typescript
import { HeartMonitor } from "@nibiruchain/nibijs"

const hm = new HeartMonitor("https://hm-graphql.devnet-2.nibiru.fi/query")
// Or omit to use default endpoint
```

### Method signatures

- **user**: `HeartMonitor.user(args: GQLQueryGqlUserArgs, fields: DeepPartial<GQLUser>)` → `Promise<GqlOutUser>`.  
  **GqlOutUser** has shape `{ user?: GQLUser }`. Use **args** to pass `where: { address: "nibi1..." }`.

- **users**: `HeartMonitor.users(args: GQLQueryGqlUsersArgs, fields: DeepPartial<GQLUser>)` → `Promise<GqlOutUsers>`.  
  **GqlOutUsers** has shape `{ users?: GQLUser[] }`. **args** can include `where`, `order_by`, `order_desc`, `limit`. The SDK defaults for **users** (in `users.ts`) set `limit: 100`, `order_desc: true`, and `order_by: GQLUserOrder.GQLCreatedBlock` when not provided.

### Example: get one user's address and balances

Prefer **`all_balances`** (with **token_info**) when **`balances`** is empty or incomplete:

```typescript
const out = await hm.user(
  { where: { address: "nibi1..." } }, // or "0x..." EVM address
  {
    address: true,
    balances: { denom: true, amount: true },
    all_balances: {
      amount: true,
      token_info: {
        type: true,
        name: true,
        symbol: true,
        bank_denom: true,
        erc20_contract_address: true,
        decimals: true,
        verified: true,
        price: true,
        logo: true,
      },
    },
    created_block: { block: true },
    is_blocked: true,
  }
)
const user = out.user
// user?.address, user?.balances, user?.all_balances, user?.created_block, user?.is_blocked
```

### Example: list users with limit

```typescript
const out = await hm.users(
  { where: {}, limit: 50, order_by: GQLUserOrder.GQLAddress, order_desc: false },
  { address: true, balances: { denom: true, amount: true } }
)
const list = out.users ?? []
```

Generated types **GQLUser**, **GQLToken**, **GQLUserFilter**, **GQLUsersFilter**, **GQLUserOrder**, **GQLQueryGqlUserArgs**, **GQLQueryGqlUsersArgs** live in `nibi-ts-sdk/src/gql/utils/generated.ts`. Query builders and helpers: `nibi-ts-sdk/src/gql/query/user.ts`, `nibi-ts-sdk/src/gql/query/users.ts`, `nibi-ts-sdk/src/gql/heart-monitor/heart-monitor.ts`.

---

## 8. Gotchas

- **`balances` vs `all_balances`**: Do not assume **`balances`** is complete. If it is empty but the wallet holds tokens on chain, query **`all_balances`** with **`token_info`** (and compare with **`nibid query bank balances`** / EVM **`balanceOf`** if needed).
- **Indexer lag**: The indexer indexes blocks asynchronously. Data may be a few seconds behind the chain. For strict consistency, use the chain (e.g. `nibid query bank balances <address>`).
- **Token.amount and Balance.amount are String**: Parse for display or arithmetic. Use **TokenInfo.decimals** for **all_balances** rows when present.
- **user(where) returns nullable User**: If no user exists for the given address, **user** can return null. **users** always returns an array (possibly empty).
- **is_blocked**: Optional; when true, the account is blocked in the indexer.
- **created_block**: Represents the block at which the indexer first saw this account, not necessarily the chain’s “account creation” event.
- **Coverage**: Some chain-only denoms may appear only on **`balances`** or only via bank queries; **all_balances** is the right default for enriched / EVM-associated balances in the indexer.

---

## 9. File reference (by repo)

### Server (nibi-go-hm)

- `graphql/graph/user.graphqls`: **User**, **UserFilter**, **UsersFilter**, **UserOrder**.
- `graphql/graph/common.graphqls`: **Token**.
- `graphql/graph/query.graphqls`: Root **user** and **users** field definitions.
- `graphql/graph/block.graphqls`: **Block** (used by **User.created_block**).
- `graphql/graph/db/utils.go`: Default and max limit (1000) for pagination.

### Client (nibi-ts-sdk)

- `src/gql/heart-monitor/heart-monitor.ts`: **HeartMonitor** class and **user** / **users** methods.
- `src/gql/query/user.ts`: **user** query string builder and **user()**; **GqlOutUser**.
- `src/gql/query/users.ts`: **users** query string builder and **users()**; **GqlOutUsers**; default args.
- `src/gql/utils/generated.ts`: **GQLUser**, **GQLToken**, **GQLBalance**, **GQLTokenInfo**, **GQLUserFilter**, **GQLUsersFilter**, **GQLUserOrder**, **GQLQueryGqlUserArgs**, **GQLQueryGqlUsersArgs**, **GQLBlock** (exact names may vary by SDK version; introspect or read generated types).
