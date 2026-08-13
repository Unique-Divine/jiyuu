---
name: nibiru-gql
description: >-
  Query and investigate Nibiru's Heart Monitor (HM) GraphQL API and its indexed
  database evidence. Use for HM, Nibiru GraphQL, user or token balances,
  staking, validators, delegations, transaction hashes, Cosmos messages, custom
  Wasm events, indexing checks, or reconciliation between Heart Monitor and Sai
  Keeper. Read the relevant child reference before querying.
metadata:
  tags: ["nibiru", "heart-monitor", "graphql", "indexer"]
  agent_skills:
    - nibiru-cli-nibid
    - sai-db
    - sai-perps
---

# Nibiru GraphQL and Heart Monitor

Use this skill for data owned by the Heart Monitor (HM), Nibiru's blockchain
indexer and GraphQL API. The HM is not the Sai Keeper. It provides the
chain-event and wallet/indexer evidence that Sai Keeper consumes.

## Endpoints

| Network | GraphQL endpoint | Playground |
| --- | --- | --- |
| Mainnet | `https://hm-graphql.nibiru.fi/query` | `https://hm-graphql.nibiru.fi/graphql` |
| Testnet | `https://hm-graphql.itn-2.nibiru.fi/query` | `https://hm-graphql.itn-2.nibiru.fi/graphql` |

## Reference router

| Need | Read |
| --- | --- |
| Validators, delegations, redelegations, unbondings, rewards boundaries | [`db-staking.md`](db-staking.md) |
| Account, bank, and enriched EVM balances | [`db-user-balances.md`](db-user-balances.md) |
| Cosmos transaction messages and custom Wasm events | [`db-events.md`](db-events.md) |

## Source boundaries

```text
Nibiru chain
  -> Heart Monitor database and GraphQL
  -> Sai Keeper reads HM event data and refreshes Sai contract state
  -> Sai Keeper database and GraphQL
  -> Sai application
```

Use the owning layer for each fact:

- Heart Monitor owns indexed chain messages, custom Wasm events, and ordinary
  chain account balances.
- Sai Perp contracts own live deposits and trading credits.
- Sai Keeper owns its derived Perp state, including table
  `sai_user_deposit_balances`, and exposes that state through its GraphQL API.
- The Sai application is a client of Sai Keeper; a UI mismatch does not prove an
  HM or Sai Keeper indexing failure.

For a Sai incident, first prove the contract state with agent skill
`sai-perps`, then verify the chain event through this skill. Use agent skill
`sai-db` to locate the API-serving Sai Keeper database and inspect its derived
row. Finally, query the Sai Keeper GraphQL API to verify app-visible data.

## Routing boundaries

- Use agent skill `nibiru-cli-nibid` for authoritative chain state, address
  normalization, and transaction lookup.
- Use agent skill `sai-perps` for Perp contract state and reconciliation across
  contracts, Sai Keeper GraphQL, REST, and operator records.
- Use agent skill `sai-db` for safe discovery and read-only inspection of the
  API-serving Sai Keeper PostgreSQL database. Do not infer its endpoint from a
  database name or a remembered private IP.

## Common rules

- Prefer GraphQL field `user.all_balances` to legacy field `user.balances` for
  wallet balance investigations.
- Use GraphQL container `staking` for staking queries; root-level staking fields
  are deprecated.
- HM is asynchronous. For strict, height-specific chain state, use the chain
  rather than the indexer.
- Token and balance amounts are base units. Interpret them with the relevant
  decimal metadata.
- Distinguish an HM event from a Sai Keeper state row. A present event establishes
  that HM indexed the chain event; it does not by itself establish that a Keeper
  refresh completed.
