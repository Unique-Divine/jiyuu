# Nibiru Indexer (Heart Monitor) — Staking Schema Reference

This document describes the Nibiru indexer (Heart Monitor) and the staking portion of its GraphQL schema: what the indexer is, how the TypeScript SDK exposes it, the lineage of data from the chain, and the types/fields available for staking queries.

- [Repo Definitions](#repo-definitions)
- [1. What the indexer is and how itΓÇÖs exposed](#1-what-the-indexer-is-and-how-its-exposed)
- [2. Heart Monitor abstraction in nibi-ts-sdk](#2-heart-monitor-abstraction-in-nibi-ts-sdk)
  - [Import and Construction](#import-and-construction)
  - [Method Signature (IHeartMonitor)](#method-signature-iheartmonitor)
  - [Query Builder Pattern](#query-builder-pattern)
  - [Batching](#batching)
- [3. GraphQL Root and the Staking Container](#3-graphql-root-and-the-staking-container)
- [4. Field Discovery](#4-field-discovery)
  - [Example: List delegations by address](#example-list-delegations-by-address)
- [5. Staking type: sub-queries and arguments](#5-staking-type-sub-queries-and-arguments)
  - [Filters (Summary)](#filters-summary)
  - [Order enums (sort keys)](#order-enums-sort-keys)
- [6. Pagination & Limits](#6-pagination--limits)
- [7. Lineage & Dependency Flow (Go ΓåÆ Indexer ΓåÆ TS)](#7-lineage--dependency-flow-go-%E2%86%92-indexer-%E2%86%92-ts)
  - [The Lineage](#the-lineage)
  - [Key Chain Types (internal/cosmos-sdk)](#key-chain-types-internalcosmos-sdk)
  - [Key Indexer Types (nibi-go-hm models)](#key-indexer-types-nibi-go-hm-models)
- [8. CLI Mapping (nibid Γåö Indexer)](#8-cli-mapping-nibid-%E2%86%94-indexer)
  - [Validator addresses (valoper)](#validator-addresses-valoper)
- [9. GraphQL Schema Types (Heart Monitor)](#9-graphql-schema-types-heart-monitor)
  - [Validator](#validator)
  - [Delegation](#delegation)
  - [Redelegation](#redelegation)
  - [Unbonding](#unbonding)
  - [User](#user)
  - [StakingHistoryItem](#stakinghistoryitem)
  - [ValidatorDescription](#validatordescription)
  - [Block](#block)
- [10. Gotchas & Caveats](#10-gotchas--caveats)
  - [Compensation & Slashing](#compensation--slashing)
  - [Migration](#migration)
  - [General](#general)
- [11. Future Skills Roadmap](#11-future-skills-roadmap)
- [12. File Reference (By Repo)](#12-file-reference-by-repo)
  - [Client (`nibi-ts-sdk`)](#client-nibi-ts-sdk)
  - [Server (`nibi-go-hm`)](#server-nibi-go-hm)

## Repo Definitions

For repo locations and cross-repo routing (nibi-go-hm, nibi-ts-sdk, nibi-chain,
etc.), see the **repo-map** skill. It maps which repo to use for indexer schema
changes, SDK client code, chain logic, and how they connect.

---

## 1. What the indexer is and how it’s exposed

- **Heart Monitor** (or “hm”) is the server that indexes the blockchain and exposes that data over **GraphQL**.
- **Implementation**: The server is implemented in the `nibi-go-hm` repo.
- **Endpoint**: Clients send POST requests to the GraphQL endpoint at path `/query`. **Production URLs**: Mainnet `https://hm-graphql.nibiru.fi/query`; testnet `https://hm-graphql.itn-2.nibiru.fi/query`. **Env** is the hostname segment that identifies the network: omit it for mainnet; use `itn-2` for testnet. Generic form: `https://hm-graphql.<env>.nibiru.fi/query` (for mainnet, use `hm-graphql.nibiru.fi` with no `<env>.` segment). The playground is at `/graphql` on the same host.
- **Scope**: Staking (validators, delegations, redelegations, unbondings, history) is one module. The same API serves governance, oracle, inflation, user balances, IBC, wasm, and more.
- **Rewards**: **Rewards for a delegator are NOT from the indexer**; they come from the chain’s distribution module. Use the Nibiru CLI: `nibid query distribution` (e.g. `rewards`) and `nibid tx distribution`.

---

## 2. Heart Monitor abstraction in nibi-ts-sdk

The TypeScript SDK (`@nibiruchain/nibijs` from the `nibi-ts-sdk` repo) provides a typed client for the Heart Monitor API.

### Import and Construction
```typescript
import { HeartMonitor } from "@nibiruchain/nibijs"

const hm = new HeartMonitor("https://hm-graphql.devnet-2.nibiru.fi/query")
// Or omit to use default endpoint
```

### Method Signature (IHeartMonitor)
```typescript
readonly staking: (
  args: QueryStakingArgs,
  fields: DeepPartial<GQLStakingFields>
) => Promise<GqlOutStaking>
```
- **args**: Filters, order, limit, offset per sub-query (delegations, validators, history, etc.). Pass `{}` or omit for defaults.
- **fields**: Object shape specifying which sub-queries and nested fields to request. Keys like `delegations`, `validators`; nested values describe the selection set.
- **Return**: `GqlOutStaking` with shape `{ staking?: { delegations?, validators?, ... } }`.

### Query Builder Pattern
The SDK turns `args` + `fields` into a GraphQL document via:
- `QueryStakingArgs`: Per-sub-query arguments.
- `GQLStakingFields`: Selection set for the sub-queries.
- `convertObjectToPropertiesString()`: Recursively converts the `fields` object to a GraphQL selection string.
- `gqlQuery()`: Builds `fieldName(args) { selection }`.
- `doGqlQuery()`: POSTs the query to the endpoint.

### Batching
The SDK exposes `GQLQueryGqlBatchHandler`: given an array of query strings, it merges them into one document `{ query1 query2 ... }` and runs a single POST. Staking can be requested alone or as part of a larger batch with other root fields (communityPool, inflation, proxies, etc.).

---

## 3. GraphQL Root and the Staking Container

Root type `Query` has a field **`staking: Staking!`**. Use the **container pattern**:

```graphql
query {
  staking {
    validators(...) { ... }
    delegations(...) { ... }
    redelegations(...) { ... }
    unbondings(...) { ... }
    history(...) { ... }
  }
}
```

**CRITICAL**: Root-level fields like `validators`, `delegations`, `unbondings`, `redelegations` exist on `Query` but are **deprecated** ("Moved to staking sub schema"). Always use `query { staking { ... } }`.

---

## 4. Field Discovery

- **Schema as source of truth**: Read `.graphqls` files in `nibi-go-hm/graphql/graph/`. They define types, filters, enums, and arguments.
- **Generated TS types**: `nibi-ts-sdk/src/gql/utils/generated.ts` contains `GQLValidator`, `GQLDelegation`, etc. that mirror the schema.
- **Introspection**: You can use GraphQL introspection (`__schema`) or `featureFlags` to check module availability at runtime.

### Example: List delegations by address
To list delegations for a given delegator address:

```graphql
query {
  staking {
    delegations(where: { delegator_address: "nibi1..." }, limit: 100) {
      amount
      delegator { address }
      validator { operator_address }
    }
  }
}
```

Equivalent `hm.staking()` call (TS SDK):

```typescript
const resp = await hm.staking(
  { delegations: { where: { delegator_address: "nibi1..." }, limit: 100 } },
  { delegations: { amount: true, delegator: { address: true }, validator: { operator_address: true } } }
)
// resp.staking?.delegations
```

---

## 5. Staking type: sub-queries and arguments

The **`Staking`** type is a container for five sub-queries.

| Field | Description and signature |
|-------|----------------------------|
| `validators` | `(limit, offset, order_by: ValidatorOrder, order_desc, where: ValidatorFilter): [Validator!]!` — list of validators with optional filter/sort. |
| `delegations` | `(limit, offset, order_by: DelegationOrder, order_desc, where: DelegationFilter): [Delegation!]!` — delegations; e.g. filter by `delegator_address` or `validator_address`. |
| `redelegations` | `(limit, offset, order_by: RedelegationOrder, order_desc, where: RedelegationFilter): [Redelegation!]!` — in-flight redelegations. |
| `unbondings` | `(limit, offset, order_by: UnbondingOrder, order_desc, where: UnbondingFilter): [Unbonding!]!` — in-flight unbondings. |
| `history` | `(limit, offset, order_by: StakingHistoryOrder, order_desc, where: StakingHistoryFilter): [StakingHistoryItem!]!` — staking history (delegate, unbond, redelegate, withdraw, cancel, etc.). |

### Filters (Summary)
- **DelegationFilter**: `delegator_address`, `validator_address`.
- **ValidatorFilter**: `jailed`, `moniker`, `operator_address`, `status`.
- **RedelegationFilter / UnbondingFilter**: delegator and validator addresses.
- **StakingHistoryFilter**: `delegator`, `validator`, `actions`, `amount`, `block`, `completionTime`.
- **Common Filters**: See `common.graphqls` for `IntFilter`, `TimeFilter`, `StringFilter` (contains, eq, in, etc.).

### Order enums (sort keys)
Use `order_by` and `order_desc` on sub-queries to sort results. Values:

- **ValidatorOrder**: `operator_address`, `jailed`, `status`, `moniker`, `tokens`
- **DelegationOrder**: `validator_address`, `delegator_address`
- **UnbondingOrder**: `validator_address`, `delegator_address`, `creation_height`, `completion_time`
- **RedelegationOrder**: `source_validator_address`, `destination_validator_address`, `delegator_address`, `creation_height`, `completion_time`
- **StakingHistoryOrder**: `sequence`, `validator`, `delegator`, `amount`, `action`

Canonical definitions: `nibi-go-hm/graphql/graph/staking.graphqls`. TS SDK: `GQLValidatorOrder`, `GQLDelegationOrder`, etc. in `generated.ts`.

---

## 6. Pagination & Limits

In the server (`nibi-go-hm/graphql/graph/db/utils.go`):
- **`DEFAULT_LIMIT`**: 1000
- **`MAX_LIMIT`**: 1000

**Rules**:
- Requests without `limit` get 1000 rows.
- `limit` values greater than 1000 are clamped to 1000.
- **Strategy**: Use `offset` to page (0, 1000, 2000, …) until a page returns fewer rows than requested.

---

## 7. Lineage & Dependency Flow (Go → Indexer → TS)

**Shares**: A delegator's proportional claim on a validator's bonded pool. The chain stores delegation in shares; the shares↔tokens rate changes with slashing, so token amount cannot be converted back to shares after the fact.

Understanding where data comes from helps reason about schema differences (e.g., why the indexer has `amount` but not `shares`).

### The Lineage
1. **Chain State (Go)**: The Cosmos SDK `staking` module defines the canonical shape. `Delegation` in state only stores `Shares`.
2. **Indexer Sync**: `nibi-go-hm` consumes chain data (via `ValidatorDelegations` → `DelegationResponse`). It writes to the **database** using types derived from Go definitions.
3. **Indexer Database**: The indexer stores the token `Balance.Amount` at sync time.
4. **GraphQL Schema**: Reflects the DB types (e.g., `Delegation.amount` is an `Int`).
5. **TS SDK**: Generates types and query builders from the GraphQL schema.

### Key Chain Types (internal/cosmos-sdk)
```go
// Delegation in state (shares only)
type Delegation struct {
    DelegatorAddress string
    ValidatorAddress string
    Shares           math.Dec // used in compensation formula
}

// DelegationResponse (what queries return)
type DelegationResponse struct {
    Delegation Delegation
    Balance    types.Coin   // amount = val.TokensFromShares(delegation.Shares)
}

// Validator
type Validator struct {
    OperatorAddress string
    Tokens          math.Int // total tokens
    DelegatorShares math.Dec // denominator for compensation
    Jailed          bool
    Status          BondStatus
}
```

### Key Indexer Types (nibi-go-hm models)
- **Delegation**: Has `ValidatorAddress`, `DelegatorAddress`, `Amount` (int64). **No shares.** `Amount` is populated from `DelegationResponse.Balance.Amount` at index time.
- **Validator**: Has `OperatorAddress`, `Jailed`, `Status`, `Tokens`, `DelegatorShares` (float64).

---

## 8. CLI Mapping (nibid ↔ Indexer)

| CLI command (`nibid q staking ...`) | Chain Query | Indexer Equivalent |
|------------------------------------|-------------|--------------------|
| `validator <valoper>` | `Validator` | `staking { validators(where: { operator_address }) }` |
| `delegations-to <valoper>` | `ValidatorDelegations` | `staking { delegations(where: { validator_address }) }` |
| `delegations <delegator>` | `DelegatorDelegations` | `staking { delegations(where: { delegator_address }) }` |
| `delegation <delegator> <valoper>` | `Delegation` (single) | Filter `delegations(where: { delegator_address, validator_address })` |
| `redelegations` | `Redelegations` | `staking { redelegations(...) }` |
| `unbonding-delegations` | `UnbondingDelegations` | `staking { unbondings(...) }` |

**Pagination Comparison**:
- **Chain**: `--limit` (default 100), `--page-key`.
- **Indexer**: `limit` (max 1000), `offset` (0/1000/2000).

### Validator addresses (valoper)

**Valoper** is the validator operator address (Bech32 prefix `nibivaloper`, e.g. `nibivaloper1...`). It is used in staking queries and indexer filters (`operator_address`, `validator_address`).

Validators have **three** valid Bech32 address types for the same logical validator:

- **Account** — `nibi1...` — The validator's wallet/account address.
- **Operator (valoper)** — `nibivaloper1...` — Used for staking identity, indexer filters, and CLI (e.g. `nibid q staking validator <valoper>`).
- **Consensus** — `nibivalcons1...` — Used in consensus messages, block signing, and oracle/slashing logs.

Use `nibid debug addr <address>` to convert between forms; it shows Bech32 Acc, Val, and Con for any of the three. See e.g. validator incident/slashing docs for examples of all three per validator (e.g. Cogwheel, Roomit).

---

## 9. GraphQL Schema Types (Heart Monitor)

This section describes the **GraphQL types** defined in the Heart Monitor schema (primarily in `staking.graphqls`). These types represent the indexed blockchain data available for query. While they mirror the underlying database models and chain state, they are optimized for client consumption (e.g., using token `amount` instead of internal `shares`).

### Validator
Represents the state and metadata of a node operator. This includes their bonded status, voting power (tokens), commission rates, and uptime.

- `operator_address`: The validator operator address (valoper; see Validator addresses in §8) (String).
- `tokens`: Total tokens bonded to the validator in base units, e.g., `unibi` (Int).
- `status`: The validator's bonding status (Enum: `BONDED`, `UNBONDED`, `UNBONDING`).
- `description`: Metadata including moniker, website, and identity (`ValidatorDescription`).
- `commission_rates`: The current, max, and max change rates for commissions (`ValidatorCommission`). **ValidatorCommission**: `rate` (current commission), `max_rate` (ceiling), `max_change_rate` (max change per update).
- `commission_update_time`: Timestamp of the last commission change (String).
- `delegator_shares`: The total shares issued to delegators (Float). Used as the denominator in compensation math.
- `jailed`: Whether the validator is currently excluded from the consensus set (Boolean).
- `uptime`: The percentage of blocks signed in the recent window (Optional Float).
- `self_delegation`: The amount of tokens the operator has delegated to themselves (Int).
- `min_self_delegation`: The minimum tokens required for the validator to remain active (Int).
- `unbonding_time`: The time at which the validator finishes unbonding (String).
- `unbonding_block`: The block height at which the validator finishes unbonding (`Block`).
- `creation_block`: The block height at which the validator was created (`Block`).

### Delegation
Represents the relationship and token balance between a delegator and a specific validator.

- `amount`: The current token balance of the delegation in base units (Int). Derived from shares at index time.
- `delegator`: The account holding the delegation (`User`).
- `validator`: The validator receiving the delegation (`Validator`).

### Redelegation
Represents tokens in the process of being moved from one validator to another.

- `amount`: The amount of tokens being redelegated (Int).
- `completion_time`: When the redelegation will finish (String).
- `creation_block`: The block height where the redelegation started (`Block`).
- `delegator`: The account performing the redelegation (`User`).
- `source_validator`: The validator tokens are moving from (`Validator`).
- `destination_validator`: The validator tokens are moving to (`Validator`).

### Unbonding
Represents tokens in the process of being withdrawn from a validator.

- `amount`: The amount of tokens being unbonded (Int).
- `completion_time`: When the tokens will become liquid (String).
- `creation_block`: The block height where the unbonding started (`Block`).
- `delegator`: The account unbonding tokens (`User`).
- `validator`: The validator tokens are being removed from (`Validator`).

### User
Represents a Nibiru account/address and its associated indexer-tracked metadata.

```graphql
type User @goModel(model: "heartmonitor/graphql/graph/model.User") {
  address: String!
  balances: [Token]!
  created_block: Block!
  is_blocked: Boolean
}
```

- `address`: The Bech32 account address (`nibi1...`).
- `balances`: List of token balances for the user.
- `created_block`: The block height where the account was first seen by the indexer.
- `is_blocked`: Whether the account is currently blocked in the indexer.

### StakingHistoryItem
A record of a discrete staking event. Useful for auditing user activity over time.

- `action`: The type of staking event (Enum: `delegate`, `unbond`, `redelegate`, `withdraw`, `cancel`, etc.).
- `amount`: The token amount associated with the action (Int).
- `block`: The block height and timestamp of the event (`Block`).
- `completion_time`: The expected completion time for delayed actions (Optional Time).
- `delegator`: The account that initiated the action (`User`).
- `validator`: The primary validator involved in the action (`Validator`).
- `destination_validator`: The target validator for `redelegate` actions (Optional `Validator`).

### ValidatorDescription
Human-readable metadata for a validator.

- `moniker`: The validator's display name (String).
- `website`: Official website URL (String).
- `details`: Additional information or bio (String).
- `identity`: Keybase or other identity signature (String).
- `security_contact`: Email or contact info for security issues (String).

### Block
Common metadata for a specific block height.

- `block`: The block height (Int).
- `block_ts`: The UNIX timestamp of the block (Int).
- `block_duration`: Time taken to produce the block (Float).
- `num_txs`: Number of transactions in the block (Int).

**Units**: All token amounts (`amount`, `tokens`, `self_delegation`, etc.) are returned as **Int** in base units (e.g., `unibi`). To display as `NIBI`, divide by $10^6$.

---

## 10. Gotchas & Caveats

### Compensation & Slashing
**Compensation** here means reimbursing delegators after slashing or validator misbehavior — not to be confused with **commission** (the validator's share of rewards).

- **Shares required**: Compensation formulas need `delegation.shares` and `validator.delegator_shares`. 
- **Indexer Limit**: The indexer has `amount` (balance), not shares. You cannot derive shares from amount because the exchange rate changes with slashing. 
- **Snapshot accuracy**: Indexer can lag chain state. For reproducible compensation at a specific slash height, use chain queries with `--height`.
- **Bond Denom**: The staking token denomination is **unibi** — micro NIBI (μ NIBI), where "u" comes from "μ" (mu). Check `nibid query staking params` to confirm on-chain.

### Migration
- **Blocked entries**: In-flight `redelegations` and `unbondings` can block forced migrations.
- **Filtering**: Use `Validator.jailed` and `delegations(where: { validator_address })` to identify affected delegators.

### General
- **Use the container**: Only use `query { staking { ... } }`.
- **Lag**: The indexer indexes blocks asynchronously. Data may be a few seconds behind the chain.

---

## 11. Future Skills Roadmap

Other API areas follow the same pattern (Container → Sub-queries → Types).

| Module | Schema File | Focus Area |
|--------|-------------|------------|
| **governance** | `governance.graphqls` | Proposals, deposits, votes |
| **oracle** | `oracle.graphqls` | Prices, entries |
| **wasm** | `wasm.graphqls` | Contracts, code, instances |
| **user** | `user.graphqls` | Balances, messages |
| **inflation** | `inflation.graphqls` | Inflation data |
| **distribution**| `distribution.graphqls`| Commissions |

---

## 12. File Reference (By Repo)

### Client (`nibi-ts-sdk`)
- `src/gql/heart-monitor/heart-monitor.ts`: `HeartMonitor` class and interfaces.
- `src/gql/query/staking.ts`: Staking query builder and types (`QueryStakingArgs`, `GQLStakingFields`).
- `src/gql/utils/consts.ts`: Query utilities (`doGqlQuery`, `gqlQuery`, `convertObjectToPropertiesString`).
- `src/gql/utils/schema.graphql`: Local copy of the schema.
- `src/gql/utils/generated.ts`: Generated types (GQLValidator, etc.).

### Server (`nibi-go-hm`)
- `graphql/graph/query.graphqls`: Root Query definition.
- `graphql/graph/staking.graphqls`: Staking-specific types and filters.
- `graphql/graph/common.graphqls`: Common filters (IntFilter, etc.).
- `graphql/graph/resolver/staking.resolvers.go`: Staking resolvers.
- `graphql/graph/db/staking.go`: DB queries for staking.
- `graphql/graph/db/utils.go`: `DEFAULT_LIMIT`, `MAX_LIMIT`.

---
