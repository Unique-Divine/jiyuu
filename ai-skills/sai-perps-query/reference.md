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
- [Fee config validation](#fee-config-validation)

## Mainnet addresses

| Contract | Address |
|---|---|
| **Perp** | `nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph` |
| **Oracle** | `nibi1xfwyfwtdame6645lgcs4xvf4u0hpsuvxrcelfwtztu0pv7n4l6hqw5a8gj` |
| SLP-USDC Vault, Group 0 | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault, Group 0 | `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej` |
| SLP-USDC Vault, Group 1 | `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8` |
| SLP-stNIBI Vault, Group 1 | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |
| SLP-USDC Vault, Group 2 | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault, Group 2 | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |
| SLP-USDC Vault, Group 3 | `nibi1waf5c8z55qvjay4de8wkm9cxyt6wa8zdnrvlexjrq77lqgqf258q3yn7l8` |
| SLP-stNIBI Vault, Group 3 | `nibi1pgurgas0za436c3fm2km99zkzutfx0jwpn7meespv6szv8c8g39qjz2tvj` |
| SLP-USDC Vault, Group 4 | `nibi193m2a00pmdsvkcvugrfewqzhtq6k0srkjzvxp2sk357vlpspx5vqxu8d7p` |
| SLP-stNIBI Vault, Group 4 | `nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej` |

**Group meanings (Perp `GroupIndex`):**
- `GroupIndex(0)` = main crypto markets
- `GroupIndex(1)` = real estate (coded estate)
- `GroupIndex(2)` = exotic
- `GroupIndex(3)` = watch assets
- `GroupIndex(4)` = equities and commodities

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
| 51 | `watch-index` | 1 |
| 52 | `rolex` | 1 |
| 53 | `patek-phillippe` | 1 |
| 54 | `audemars-piguet` | 1 |
| 55 | `omega` | 1 |
| 56 | `cartier` | 1 |
| 57 | `breitling` | 1 |
| 58 | `tudor` | 1 |
| 59 | `qqq` | 1 |
| 60 | `spy` | 1 |
| 61 | `nvda` | 1 |
| 62 | `amd` | 1 |
| 63 | `intc` | 1 |
| 64 | `msft` | 1 |
| 65 | `aapl` | 1 |
| 66 | `googl` | 1 |
| 67 | `amzn` | 1 |
| 68 | `meta` | 1 |
| 69 | `avgo` | 1 |
| 70 | `tsla` | 1 |
| 71 | `brk-b` | 1 |
| 72 | `coin` | 1 |
| 73 | `jpm` | 1 |
| 74 | `pltr` | 1 |
| 75 | `qbts` | 1 |
| 76 | `qubt` | 1 |
| 77 | `xau-usd` | 1 |
| 78 | `xag-usd` | 1 |
| 79 | `xpt-usd` | 1 |
| 80 | `xpd-usd` | 1 |
| 81 | `wti-usd` | 1 |
| 82 | `xbr-usd` | 1 |
| 83 | `ng-usd` | 1 |
| 84 | `gme` | 1 |

## MarketIndex reference (mainnet)

Every market has the following fields.
- `quote: TokenIndex(0)` (USD placeholder)
- `spread_p: 0`
- `fee_index` varies by market

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
| 49 | 50 | `pokemon-card-index` | 0 | `GroupIndex(2)` | `FeeIndex(1)` |
| 50 | 51 | `watch-index` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 51 | 52 | `rolex` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 52 | 53 | `patek-phillippe` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 53 | 54 | `audemars-piguet` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 54 | 55 | `omega` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 55 | 56 | `cartier` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 56 | 57 | `breitling` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 57 | 58 | `tudor` | 0 | `GroupIndex(3)` | `FeeIndex(1)` |
| 1000 | 59 | `qqq` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1001 | 60 | `spy` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1002 | 61 | `nvda` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1003 | 62 | `amd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1004 | 63 | `intc` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1005 | 64 | `msft` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1006 | 65 | `aapl` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1007 | 66 | `googl` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1008 | 67 | `amzn` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1009 | 68 | `meta` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1010 | 69 | `avgo` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1011 | 70 | `tsla` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1012 | 71 | `brk-b` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1013 | 72 | `coin` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1014 | 73 | `jpm` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1015 | 74 | `pltr` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1016 | 75 | `qbts` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1017 | 76 | `qubt` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1018 | 77 | `xau-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1019 | 78 | `xag-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1020 | 79 | `xpt-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1021 | 80 | `xpd-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1022 | 81 | `wti-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1023 | 82 | `xbr-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1024 | 83 | `ng-usd` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |
| 1025 | 84 | `gme` | 0 | `GroupIndex(4)` | `FeeIndex(0)` |

---

## Fee config validation

Use on-chain smart queries to verify deployed fee parameters. For fee
**semantics** (what each fee means, distribution splits, code paths), see
`/epics/sai/26-05-26-sai-perpetuals-fee-analysis.md`.

Use `sai_perps_q` from `SKILL.md`. Index wrappers must match mainnet:
`"MarketIndex(0)"`, `"FeeIndex(0)"`, not `{"0": 0}`.

### Effective fee formula

```txt
effective_fee = base_fee × min(tier_multiplier, referral_discount_multiplier)
```

Query `get_trader_fee_multiplier` for the combined result for a specific trader.

### Default base fees (`FeeIndex(0)`)

| Field | Expected default |
|---|---|
| `open_fee_p` | `"0.02"` (2.0%) |
| `close_fee_p` | `"0.015"` (1.5%) |
| `trigger_order_fee_p` | `"0.03"` (3.0%) |
| `min_position_size_usd` | `"1"` |

Resolve the fee index from the market first:

```bash
sai_perps_q "$PERP" '{"get_market":{"index":"MarketIndex(0)"}}'
# → fee_index: "FeeIndex(0)" (most markets)

sai_perps_q "$PERP" '{"get_fees":{"index":"FeeIndex(0)"}}'
```

### Default fee tiers (`get_fee_tiers`)

Eight tiers; multipliers decrease as point thresholds increase:

| Tier | `points_treshold` | `fee_multiplier` |
|---:|---:|---:|
| 0 | 6_000_000 | 0.975 |
| 1 | 20_000_000 | 0.950 |
| 2 | 50_000_000 | 0.925 |
| 3 | 100_000_000 | 0.900 |
| 4 | 250_000_000 | 0.850 |
| 5 | 400_000_000 | 0.800 |
| 6 | 1_000_000_000 | 0.700 |
| 7 | 2_000_000_000 | 0.600 |

```bash
sai_perps_q "$PERP" '{"get_fee_tiers":{}}'
sai_perps_q "$PERP" '{"get_trader_fee_multiplier":{"trader":"nibi1..."}}'
# → "1.0" (no discount), "0.95" (referral), or lower (tier)
sai_perps_q "$PERP" '{"get_pending_gov_fees":{"index":1}}'  # USDC
```

### Validation checklist

- [ ] **Base fees:** `get_fees` for each distinct `fee_index` in use
  (`FeeIndex(0)` crypto/equities, `FeeIndex(1)` exotic/watch).
- [ ] **Fee tiers:** `get_fee_tiers` returns 8 tiers with expected defaults.
- [ ] **Trader multipliers:** `get_trader_fee_multiplier` for a new trader
  (`1.0`), referred trader (`< 1.0`), and high-volume trader (tier discount).
- [ ] **Market wiring:** each `get_market` response has a valid `fee_index`.

### Not queryable on-chain (workarounds)

These live in contract state but have no smart query today. Infer from code
defaults, admin events, or controlled test trades.

| Setting | Storage key | Default | Workaround |
|---|---|---|---|
| Vault closing fee % | `VAULT_CLOSING_FEE_P` | 4.2% of closing-fee component | Close a trade; compare vault reward to closing fee charged |
| Referrer fee tiers | `REFERRER_FEE_PERCENTAGE` | 5%, 10%, 15%, 50% | Code default in `contracts/perp/src/fees/state.rs` |
| Referee discount | `REFERREE_BASE_FEE_MULTIPLIER` | 5% off base (`0.95` effective) | Compare `get_trader_fee_multiplier` with/without referrer |
| Referrer maps | `USER_REFERRERS`, `REFERRER_FEE_TIER`, `REFERRER_FEES` | — | Use `get_trader_fee_multiplier`; referral GraphQL via `sai-keeper-graphql` |

See also `slp-vaults.md` for `get_pending_gov_fees` and vault reward routing.
