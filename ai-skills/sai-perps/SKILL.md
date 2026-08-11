---
name: sai-perps
description: >-
  Query and reconcile Sai perps data across live Perp, Oracle, and SLP Vault
  contracts; Sai Keeper GraphQL; DexPal REST; sai-keeper Postgres; and referral
  records. Use for markets, open interest, fees, trades, vault health, oracle
  prices, REST statistics, database-backed metrics, referral ownership or
  history, trading-credit balances, app/indexer behavior, or mismatches between
  chain, APIs, databases, sheets, and operator records. Keep mutations and
  private operator workflows in sai-ops.
metadata:
  tags: ["sai-perps", "sai-keeper", "sai-app"]
  agent_skills:
    - nibiru-gql
    - nibiru-cli-nibid
    - postgresql-psql
    - sai-ops
---

# Sai Perps

Use this skill for Sai read/query/reconcile work across protocol state, indexed
data, public APIs, database semantics, referral analytics, and app behavior.

## Boundaries

Use agent skill `sai-ops` when a request uploads code, migrates contracts,
executes admin messages, creates or updates referral codes, issues trading
credits, changes production configuration, triggers trades, mutates a sheet, or
writes a state-change changelog. This skill can supply read-only preflight and
postflight checks.

For a Sai Keeper Postgres schema, SQL semantics, or freshness question, read
[`references/sai-db.md`](references/sai-db.md). For direct private database
access, Tailscale/primary-replica routing, or any write operation, also use the
private agent skill `sai-db`; do not infer connection details from repository
configuration or environment variables.

For Heart Monitor GraphQL transaction messages, custom Wasm events, and ordinary
chain-indexed wallet evidence, use agent skill `nibiru-gql`. Heart Monitor event
proof establishes the upstream chain input; it does not by itself prove the Sai
Keeper derived-state row or the Sai app-visible balance.

## Reference router

| Surface | Use when | Reference |
| --- | --- | --- |
| Live contracts | Protocol state, contract configuration, markets, collaterals, OI limits, fees, vault mappings, deposits, affiliate claimable rewards, and credit balances. | [`sai-contracts.md`](references/sai-contracts.md) |
| SLP Vault contracts | Vault health, collateralization, share price, deposit caps, daily risk state, epochs, withdrawal timing, raw vault state, pending gov fees. | [`sai-vaults.md`](references/sai-vaults.md) |
| Sai Keeper GraphQL | Indexed trades and LP history, app-visible oracle prices, fees, referral history, subscriptions, and user-facing entities. | [`sai-graphql.md`](references/sai-graphql.md) |
| DexPal REST | Public product aggregates, stats, markets, yield, health checks, and date-window metrics. | [`sai-rest.md`](references/sai-rest.md) |
| Sai Keeper Postgres | Table and column meanings, units, migrations, routines, API implementation, and aggregate freshness. | [`sai-db.md`](references/sai-db.md) |
| Cross-source reconciliation | Explaining why chain, GraphQL, REST, DB, sheets, or boku notes disagree. | [`sai-reconciliation.md`](references/sai-reconciliation.md) |
| Referral checks | Existing referral code ownership, redemption history, sheet metadata, EVM/Bech32 normalization, testnet balance/credit checks. | [`sai-referrals.md`](references/sai-referrals.md) |

## Diagnostic Order

For questions spanning multiple systems, prefer this order unless the prompt
explicitly selects a surface:

1. Identify the network, entity, and source-of-truth layer: contract address,
   market index, token index, trader, vault, referral code, REST endpoint,
   GraphQL field, or database table.
2. Read live contract state when the fact controls protocol behavior.
3. Read GraphQL or REST when the fact is about what the app/API exposes.
4. Use database references or SQL when GraphQL/REST semantics, freshness, or
   aggregation logic are the question.
5. Compare sources explicitly. Say which layer owns each fact and which layer is
   stale, derived, human-maintained, or not expected to match exactly.

## Common Pitfalls

- Contract query JSON uses CosmWasm `snake_case`; GraphQL fields use camelCase.
- On-chain and database amounts are often base units. GraphQL `Int` amounts are
  also base units. REST responses often expose human or USD values.
- `TokenIndex(0)` is the USD quote placeholder, not collateral. Mainnet
  collateral indices are `TokenIndex(1)` for USDC and `TokenIndex(2)` for stNIBI.
- Some contract queries require wrapped string indices like `"MarketIndex(0)"`;
  others require plain integers. Read the contract reference before guessing.
- REST 24-hour windows are not all identical. `GET /dexpal/v1/stats` and
  `GET /dexpal/v1/markets/details` can use different time anchors.
- Sheets and boku epics are operator records, not protocol truth.

## Examples and trigger tuning

Use [`examples/sai-perps-prompts.md`](examples/sai-perps-prompts.md) for
realistic prompts that should trigger this skill, plus near misses that should
use agent skill `sai-ops` or another specialized skill.
