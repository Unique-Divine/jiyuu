---
name: sai-perps-query-reference
description: Full Sai mainnet TokenIndex + MarketIndex lookup (from epic doc).
---

# Sai Perps (Mainnet): Tokens & Markets Reference

This file is the **full lookup** companion to `SKILL.md`.

- [Mainnet addresses](#mainnet-addresses)
- [Collaterals (mainnet)](#collaterals-mainnet)
- [TokenIndex reference (mainnet)](#tokenindex-reference-mainnet)
- [MarketIndex reference (mainnet)](#marketindex-reference-mainnet)

## Mainnet addresses

| Contract | Address |
|---|---|
| **Perp** | `nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph` |
| **Oracle** | `nibi1xfwyfwtdame6645lgcs4xvf4u0hpsuvxrcelfwtztu0pv7n4l6hqw5a8gj` |
| SLP-USDC Vault, Group 0. | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault, Group 0. | `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej` |
| SLP-USDC Vault, Group 1 | `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8` |
| SLP-stNIBI Vault, Group 1. | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |

**Group meanings (Perp `GroupIndex`):**
- `GroupIndex(0)` = main crypto markets
- `GroupIndex(1)` = real estate (coded estate)

## Collaterals (mainnet)

Never use `collateral_index: 0`. Mainnet collaterals are:

| TokenIndex | base | Notes |
|---:|---|---|
| 1 | `usdc` | collateral |
| 2 | `stnibi` | collateral (contract denom contains `ampNIBI`) |

---

## TokenIndex reference (mainnet)

Per the epic doc, **`TokenIndex(0)` is used by Perp as quote (USD)** and is **not**
present in Oracle (`get_token_by_id` returns "token id 0 does not exist").


| TokenIndex | base | permission_group |
|---:|---|---:|
| 0 | (USD quote placeholder; not in Oracle) | - |
| 1 | `usdc` | 2 |
| 2 | `stnibi` | 2 |
| 3 | `btc` | 2 |
| 4 | `eth` | 2 |
| 5 | `atom` | 2 |
| 6 | `dubai` | 1 |
| 7 | `los-angeles` | 1 |
| 8 | `new-york` | 1 |
| 9 | `chicago` | 1 |
| 10 | `washington` | 1 |
| 11 | `pittsburgh` | 1 |
| 12 | `miami-beach` | 1 |
| 13 | `boston` | 1 |
| 14 | `brooklyn` | 1 |
| 15 | `austin` | 1 |
| 16 | `denver` | 1 |
| 17 | `sol` | 2 |
| 18 | `xrp` | 2 |
| 19 | `sui` | 2 |
| 20 | `ena` | 2 |
| 21 | `arb` | 2 |
| 22 | `trx` | 2 |
| 23 | `apt` | 2 |
| 24 | `pol` | 2 |
| 25 | `ton` | 2 |
| 26 | `ada` | 2 |
| 27 | `ltc` | 2 |
| 28 | `doge` | 2 |
| 29 | `aster` | 2 |
| 30 | `bnb` | 2 |
| 31 | `pump` | 2 |
| 32 | `avax` | 2 |
| 33 | `aave` | 2 |
| 34 | `pengu` | 2 |
| 35 | `link` | 2 |
| 36 | `bonk` | 2 |
| 37 | `shib` | 2 |
| 38 | `trump` | 2 |
| 39 | `near` | 2 |
| 40 | `kaito` | 2 |
| 41 | `mnt` | 2 |
| 42 | `ip` | 2 |
| 43 | `virtual` | 2 |
| 44 | `hype` | 2 |
| 45 | `zec` | 2 |
| 46 | `purr` | 2 |
| 47 | `mon` | 2 |
| 48 | `fartcoin` | 2 |
| 49 | `nibi` | 2 |
| 50 | `pokemon-card-index` | 1 |

## MarketIndex reference (mainnet)

Every market has the following fields.
- `quote: TokenIndex(0)` (USD placeholder)
- `spread_p: 0`
- `fee_index: FeeIndex(0)`

Each market has base, quote (always TokenIndex(0) = USD), spread_p, group_index,
fee_index. Base token names resolved via [TokenIndex
reference](#tokenindex-reference-mainnet) above.
```rust
#[cw_serde]
pub struct MarketInfo {
    pub base: TokenIndex,
    pub quote: TokenIndex,
    pub spread_p: Decimal,
    pub group_index: GroupIndex,
    pub fee_index: FeeIndex,
}
```

All markets have `MarketInfo.quote=TokenIndex(0)` (USD).

| MarketIndex | base (TokenIndex) | base (name) | spread_p | group_index | fee_index |
|---|---|---|---|---|---|
| 0  | 3  | `btc`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 1  | 4  | `eth`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 2  | 6  | `dubai`       | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 3  | 7  | `los-angeles` | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 4  | 8  | `new-york`    | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 5  | 9  | `chicago`     | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 6  | 10 | `washington`  | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 7  | 11 | `pittsburgh`  | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 8  | 12 | `miami-beach` | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 9  | 13 | `boston`      | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 10 | 14 | `brooklyn`    | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 11 | 15 | `austin`      | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 12 | 16 | `denver`      | 0 | `GroupIndex(1)` | `FeeIndex(0)` |
| 16 | 17 | `sol`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 17 | 18 | `xrp`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 18 | 19 | `sui`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 19 | 20 | `ena`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 20 | 21 | `arb`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 21 | 22 | `trx`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 22 | 23 | `apt`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 23 | 24 | `pol`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 24 | 25 | `ton`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 25 | 26 | `ada`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 26 | 27 | `ltc`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 27 | 28 | `doge`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 28 | 29 | `aster`       | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 29 | 30 | `bnb`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 30 | 31 | `pump`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 31 | 32 | `avax`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 32 | 33 | `aave`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 33 | 34 | `pengu`       | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 34 | 35 | `link`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 35 | 36 | `bonk`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 36 | 37 | `shib`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 37 | 38 | `trump`       | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 38 | 39 | `near`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 39 | 40 | `kaito`       | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 40 | 41 | `mnt`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 41 | 42 | `ip`          | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 42 | 43 | `virtual`     | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 43 | 44 | `hype`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 44 | 45 | `zec`         | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
| 48 | 49 | `nibi`        | 0 | `GroupIndex(0)` | `FeeIndex(0)` |
