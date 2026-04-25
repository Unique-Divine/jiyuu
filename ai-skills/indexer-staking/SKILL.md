---
name: indexer-staking
description: Use when the user asks about the Nibiru indexer (Heart Monitor), staking GraphQL schema, GQLValidator, GQLDelegation, HeartMonitor, or querying staking data via GraphQL. Explains that staking is under query.staking, not root-level deprecated fields; rewards come from the chain (distribution), not the indexer; amounts are Int in base units (e.g. unibi).
---

# Nibiru Indexer (Heart Monitor) — Staking

Use this skill when working with the **Nibiru indexer / Heart Monitor** GraphQL API or the **staking** part of the schema (validators, delegations, redelegations, unbondings, history).

## Quick Facts

- **Indexer** = Heart Monitor = GraphQL API. **Endpoints** (path `/query`): Mainnet `https://hm-graphql.nibiru.fi/query`; testnet `https://hm-graphql.itn-2.nibiru.fi/query`. **Env** is the hostname segment for the network: mainnet has no env (host `hm-graphql.nibiru.fi`); testnet uses env `itn-2` (host `hm-graphql.itn-2.nibiru.fi`). The playground uses the same host with path `/graphql`.
- **Canonical Path**: Always use `query { staking { ... } }`. Root-level fields are **deprecated**.
- **Bond denom**: The staking token denomination is **unibi** — shorthand for **micro NIBI** (μ NIBI), where "u" comes from "μ" (mu).
- **Amounts**: Stored as **Int** in base units (unibi). Divide by 10^6 for `NIBI`.
- **Shares**: A delegator's proportional claim on a validator's pool; the shares-to-tokens rate changes with slashing (see Shares vs. Amount below).
- **Valoper**: Validator operator address (`nibivaloper1...`). Validators have three Bech32 identities: account `nibi1...`, operator (valoper), and consensus `nibivalcons1...`; use `nibid debug addr` to convert. Indexer/CLI staking use valoper.
- **Rewards**: **Not in the indexer**. Use the chain’s distribution module via the Nibiru CLI: `nibid query distribution` (e.g. `rewards`) and `nibid tx distribution`.
- **TS SDK**: Use `HeartMonitor` from `@nibiruchain/nibijs`.

## Dependency Flow & Lineage

Data flows from the chain to the client in a specific pipeline. Understanding this prevents schema confusion:

1.  **Chain State (Go)**: Source of truth. Staking module (`internal/cosmos-sdk`) stores **Shares** for delegations.
2.  **Indexer Sync**: The `nibi-go-hm` server queries the chain, calculates the token **Balance**, and persists it to the indexer DB.
3.  **GraphQL API**: The API (`staking.graphqls`) exposes these DB fields. This is why you see `amount` (tokens) but usually not `shares` in indexer delegations.
4.  **TS SDK**: The client (`nibi-ts-sdk`) generates types and query builders from the GraphQL schema.

## Shares vs. Amount (Critical)

**Shares** are a delegator's proportional claim on a validator's pool; the shares-to-tokens rate changes with slashing.

- **Indexer `Delegation.amount`** is a **balance snapshot** in tokens. It does not track shares.
- **Slashing/Compensation**: Here **compensation** means reimbursing delegators after slashing or validator misbehavior (distinct from **commission**, the validator's share of rewards). You **cannot** derive shares from the indexer amount after a slash. For reproducible compensation math, you **MUST** query the chain directly (e.g., `nibid q staking delegations-to`) using a specific `--height`.

## Rule of Thumb: Where to Query?

| Requirement | Preferred Source | Reason |
| :--- | :--- | :--- |
| Current delegator balances | **Indexer** | Fast, indexed, GraphQL support. |
| Historical staking actions | **Indexer** | `staking { history }` tracks events over time. |
| **Rewards** | **Chain** | Indexer does not track distribution module data. |
| **Compensation / Slashing** | **Chain** | Requires `shares` and height-specific snapshots. |
| Validator uptime/jailed status | **Either** | Indexer is fine unless absolute real-time is needed. |

## Additional Resources

- **[Full Reference (reference.md)](reference.md)**:
  - [Canonical Query Shape & Deprecation](reference.md#3-graphql-root-and-the-staking-container)
  - [TS SDK Internals & Query Builder](reference.md#2-heart-monitor-abstraction-in-nibi-ts-sdk)
  - [Pagination & 1000-Row Limits](reference.md#6-pagination--limits)
  - [Lineage & Chain Types (Go)](reference.md#7-lineage--dependency-flow-go--indexer--ts)
  - [CLI Mapping (nibid ↔ Indexer)](reference.md#8-cli-mapping-nibid--indexer)
  - [Compensation & Migration Gotchas](reference.md#10-gotchas--caveats)
  - [GraphQL Schema Types](reference.md#9-graphql-schema-types-heart-monitor)
  - [User type definition](reference.md#user)
  - [Order enums (ValidatorOrder, DelegationOrder, etc.)](reference.md#order-enums-sort-keys)
  - [Example: list delegations for address X](reference.md#example-list-delegations-by-address)
  - [Upstream Repo File Pointers](reference.md#12-file-reference-by-repo)
