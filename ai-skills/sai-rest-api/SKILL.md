---
name: sai-rest-api
description: Query the Sai REST API (DexPal) for aggregated metrics, stats, yield, markets, referrals, and health checks. Use when working with Sai REST endpoints, /dexpal/v1/*, sai-api.nibiru.fi, or when needing quick stats instead of full GraphQL.
---

# Sai: REST API Notes

This guide covers the **REST API** — DexPal aggregated metrics and feeds (`/dexpal/v1/*`, `/health`, `/`). Use it for quick stats and health checks.

- [Selecting the Right API](#selecting-the-right-api)
- [Available Endpoints](#available-endpoints)
  - [24h Time Window Semantics](#24h-time-window-semantics)
- [Usage Examples](#usage-examples)
  - [GET /](#get-)
  - [GET /dexpal/v1/stats](#get-dexpalv1stats)
  - [GET /dexpal/v1/metrics](#get-dexpalv1metrics)
  - [GET /dexpal/v1/referrals](#get-dexpalv1referrals)
  - [GET /dexpal/v1/markets](#get-dexpalv1markets)
  - [GET /dexpal/v1/markets/details](#get-dexpalv1marketsdetails)
  - [GET /dexpal/v1/yield](#get-dexpalv1yield)
  - [GET /health](#get-health)
- [References](#references)

## Selecting the Right API

The **GraphQL API** is the canonical, comprehensive Sai API (perp, lp, oracle, fee, subscriptions). Use GraphQL for the full API.

| Environment | URL |
| ----------- | --- |
| **Mainnet GraphQL** | https://sai-keeper.nibiru.fi |
| **Testnet GraphQL** | https://sai-keeper.testnet-2.nibiru.fi |
| **Mainnet** | https://sai-api.nibiru.fi |
| **Testnet** | https://sai-api.testnet-2.nibiru.fi |

**Related skill**: For the full Sai API (perp trades, LP positions, oracle prices, fees, subscriptions), use the **sai-keeper-graphql** skill.

## Available Endpoints

| Path | Description |
|------|-------------|
| [/](#get-) | API info. Lists all queries. |
| [/dexpal/v1/stats](#get-dexpalv1stats) | Detailed exchange statistics (volume 24h, OI, TVL, fees, etc.) |
| [/dexpal/v1/referrals](#get-dexpalv1referrals) | DexPal referral reports |
| [/dexpal/v1/markets/details](#get-dexpalv1marketsdetails) | Detailed market data for all trading pairs (volume 24h longs/shorts, OI, etc.) |
| [/dexpal/v1/yield](#get-dexpalv1yield) | Yield earning opportunities (LP vaults) |
| [/dexpal/v1/markets](#get-dexpalv1markets) | DexPal markets feed |
| [/dexpal/v1/metrics](#get-dexpalv1metrics) | DexPal aggregate metrics |
| [/health](#get-health) | Health check |

### 24h Time Window Semantics

- **`/dexpal/v1/stats`** (optimized path): 24h window = `NOW() - 24h` (wall clock at cache refresh)
- **`/dexpal/v1/markets/details`**: 24h window = latest block timestamp - 24h (blockchain time)

## Usage Examples

Examples use `curl -s`; the `-s` (silent) flag suppresses curl progress output so piping to `jq` works cleanly.

### GET /

**Request:**
```bash
curl -s https://sai-api.nibiru.fi/ | jq .
```

**Response shape:**
```json
{
  "service": "SAI Keeper Statistics API",
  "version": "1.0.0",
  "endpoints": {
    "/dexpal/v1/markets": "GET - DexPal markets feed",
    "/dexpal/v1/metrics": "GET - DexPal aggregate metrics",
    "/dexpal/v1/stats": "GET - Detailed exchange statistics",
    "/dexpal/v1/markets/details": "GET - Detailed market data for all trading pairs",
    "/dexpal/v1/yield": "GET - Yield earning opportunities data",
    "/dexpal/v1/referrals": "GET - DexPal referral reports",
    "/health": "GET - Health check"
  }
}
```

### GET /dexpal/v1/stats

**Request:**
```bash
# Live statistics (current window)
curl -s https://sai-api.nibiru.fi/dexpal/v1/stats | jq .

# Historical statistics for a specific date
curl -s "https://sai-api.nibiru.fi/dexpal/v1/stats?date=2026-02-25" | jq .
```

**Query Parameters:**
- `date` (optional): The date for which to retrieve statistics in `YYYY-MM-DD` format (UTC).
  - Omitted or `date=today`: Returns live exchange statistics.
  - Past date: Returns historical aggregate statistics for that specific day.
  - Future date: Returns a `400 Bad Request` error.
  - Invalid format: Returns a `400 Bad Request` error.

**Response shape:**
```json
{
  "trading_volume_24h": 2584.5,
  "trading_volume_all_time": 200418.54,
  "total_trades_24h": 11,
  "total_trades_all_time": 488,
  "open_interest": 0,
  "total_users_24h": 6,
  "total_users_all_time": 39,
  "total_open_positions": 31,
  "tvl": 357801.98,
  "accrued_trading_fees_24h": 128.86,
  "accrued_trading_fees_all_time": 1949.43
}
```
*(Note: For historical dates, `tvl` and `total_open_positions` may be `null` as they are not currently tracked in daily snapshots.)*

### GET /dexpal/v1/metrics

**Request:**
```bash
curl -s https://sai-api.nibiru.fi/dexpal/v1/metrics | jq .
```

**Response shape:**
```json
{
  "volume_24h": 2256.93,
  "open_interest_24h": 251.9,
  "fees_24h": 2.2,
  "volume_all_time": 200418.54,
  "tvl": 357801.98,
  "traders_24h": 4,
  "timestamp": "2026-02-18T23:33:04.883348856Z"
}
```

### GET /dexpal/v1/referrals

**Request:**
```bash
curl -s https://sai-api.nibiru.fi/dexpal/v1/referrals | jq .
```

**Response shape:**
```json
{
  "reports": null
}
```
(`reports` can be null or an array of report objects when data exists.)

### GET /dexpal/v1/markets

**Request:**
```bash
curl -s https://sai-api.nibiru.fi/dexpal/v1/markets | jq .
```

**Response shape:**
```json
{
  "markets": [],
  "timestamp": "2026-02-19T00:15:27.502648961Z"
}
```

### GET /dexpal/v1/markets/details

**Request:**
```bash
curl -s https://sai-api.nibiru.fi/dexpal/v1/markets/details | jq .
```

**Response shape:**
```json
{
  "markets": null
}
```
(`markets` can be null or an array of market objects with base_currency, quote_currency, trading_volume_24h_longs, trading_volume_24h_shorts, open_interest, etc.)

### GET /dexpal/v1/yield

**Request:**
```bash
curl -s https://sai-api.nibiru.fi/dexpal/v1/yield | jq .
```

**Response shape:**
```json
{
  "yield_opportunities": [
    {
      "name": "SAI Liquidity Vault - nibi1m...kfej",
      "type": "vault",
      "description": "Provide liquidity to SAI perpetual futures trading and earn yield from trading fees and protocol revenue. Epoch-based withdrawals ensure stable liquidity.",
      "network": "nibiru",
      "contract_address": "nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej",
      "accepted_deposits": ["stnibi", "usdc"],
      "tvl": 356356.08,
      "average_apy": 0.06,
      "reward_token": "NUSD",
      "reward_frequency": "per_block",
      "min_deposit": 10,
      "max_deposit": 1000000,
      "withdrawal_lockup_days": 9,
      "early_withdrawal_penalty": 0,
      "date_listed": "2025-12-19T19:29:05.788346Z",
      "details_link": "https://app.sai.zone/vault/nibi1mrplvu3scplnrgns96kg0j8pk3l2p9c7eaz0qdedx0kt3vmcujyqrjkfej"
    }
  ]
}
```

### GET /health

**Request:**
```bash
curl -s https://sai-api.nibiru.fi/health
```
(Plain text response; omit jq.)

**Response shape:**
```
OK
```

## References

- **The `sai-keeper` repo**: `$HOME/ki/sai-keeper` — root `README.md` documents stats, health, and /; full endpoint list is in `api/server.go`
- **The `nibi-iac` repo**: `$HOME/ki/nibi-iac` — `terraform/_modules/sai-keeper` for deployment

The sai-keeper Terraform module (in the `nibi-iac` repo) exposes two domains: `domain_graphql` (sai-keeper.*) for GraphQL, `domain_rest` (sai-api.*) for this REST API.

Source: the `nibi-iac` repo — mainnet in `mainnet-gcp/06-mainnet.tf` (domain_rest), testnet in `itn2-gcp/07-testnet-2.tf`. DNS: `nibiru-gcp/02-dns.tf`.

Notes on querying the Sai REST API (the `sai-keeper` repo run with `-api` flag). Endpoints from the `nibi-iac` repo; paths from the `sai-keeper` repo.
