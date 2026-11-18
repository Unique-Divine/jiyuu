---
name: sai-perps
description: >-
  Moved into agent skill `sai`. Query and reconcile Sai Perps data from live
  contracts, Sai Keeper GraphQL, and DexPal REST. Use for markets, trades,
  fees, open interest, vaults, balances, referrals, or disagreements between
  on-chain and indexed data, then load agent skill `sai` for the references.
  Use agent skill `sai-ops` for mutations.
---

# Sai Perps

This skill moved into agent skill `sai`. Load `sai` and follow its router.

| Old page in `sai-perps` | Page in `sai` |
| --- | --- |
| `references/sai-contracts.md` | [`references/contracts.md`](../sai/references/contracts.md) |
| `references/sai-vaults.md` | [`references/vaults.md`](../sai/references/vaults.md) |
| `references/sai-graphql.md` | [`references/graphql.md`](../sai/references/graphql.md) |
| `references/sai-rest.md` | [`references/rest.md`](../sai/references/rest.md) |
| `references/sai-referrals.md` | [`references/referrals.md`](../sai/references/referrals.md) |
| `references/sai-reconciliation.md` | [`references/reconciliation.md`](../sai/references/reconciliation.md) |
| `examples/sai-perps-prompts.md` | [`examples/prompts.md`](../sai/examples/prompts.md) |

Two things stay outside agent skill `sai`:

- [`references/sai-db.md`](references/sai-db.md), the Sai Keeper Postgres
  table and SQL reference, stays here. Agent skill `sai` ships inside the
  public `sai` CLI and carries no database material. For database access,
  primary and replica routing, or any write, use private agent skill
  `sai-db`.
- Contract and operator mutations belong to agent skill `sai-ops`.
