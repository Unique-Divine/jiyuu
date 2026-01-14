---
name: repo-map
description: >-
  Maps and routes between key repositories on my machine (boku, nibi-chain,
  nibi-geth, nibi-go-hm, nibi-ts-sdk, sai-website, sai-perps, nibi-iac,
  gh-io-ud, wasm-cosmwasm, wasm-go-wasmvm). Use when the user asks which repo to look at,
  where a feature lives, how repos connect, how services are deployed
  (Terraform/IaC), when a task spans chain, EVM, indexer, SDK, and frontend, or
  when the user asks about the personal site, blog posts, or Astro/gh-pages.
---

# Repo Map — Nibiru and Personal Repos

Use this skill to quickly route questions/tasks to the correct repo(s) under path `$HOME/ki/`.

## Quick routing

### Blockchain and EVM Impl
- `$HOME/ki/nibi-chain`: **Chain logic, modules, and Nibiru CLI (`nibid`)**. This repo
defines the logic, and you can run `nibid tx`, `nibid query`, or other commands in
the terminal to inspect exact functionality. Core L1 chain (Go). App wiring +
custom modules. Includes `cmd/nibid`. | `x/`, `app/`, `proto/`, `cmd/nibid/`
- `$HOME/ki/nibi-chain/x/evm`: **Nibiru EVM execution and precompiles live here.**. RPC is standard for the EVM.
- `$HOME/ki/nibi-geth`: **EVM state transition internals that do not involve precompiles; go-ethereum logic**
- `$HOME/ki/wasm-cosmwasm`: **CosmWasm repo**. Official GitHub name "CosmWasm/cosmwasm".
Defines the Wasmer version used via `cosmwasm-vm`'s `Cargo.toml`.
- `$HOME/ki/wasm-go-wasmvm`: **WasmVM repo**. Official GitHub name "CosmWasm/wasmvm".
Nibiru connects to this repo via the `github.com/CosmWasm/wasmvm` dependency in
`nibi-chain/go.mod` (currently version 1.5.9). `libwasmvm` depends on `cosmwasm`
transitively to pull in `wasmer`. The relationship is: `nibi-chain` -> `wasmvm`
-> `cosmwasm` -> `wasmer`.

### Nibiru Indexer and TypeScript SDK
- `$HOME/ki/nibi-go-hm`: 
   - **Indexer server (Heart Monitor) and GraphQL schema**. This is the Nibiru
   Indexer schema's source of truth. `graphql/graph/*.graphqls` (e.g. `staking.graphqls`, `user.graphqls`)
- `$HOME/ki/nibi-ts-sdk`: **TypeScript SDK (`@nibiruchain/nibijs`)** and Nibiru
Indexer (HeartMonitor) client. Look for `HeartMonitor`, generated `GQL*` types,
and query builders.

### Sai Exchange
- `$HOME/ki/sai-website/webapp`: **Sai: web app** (lives in monorepo). Web
frontend and API integrations. This uses the "sai-keeper" GraphQL interface.
- `$HOME/ki/sai-perps`: **Official Sai protocol contracts** (EVM + Wasm). This repo defines what gets deployed onchain; the web app and sai-keeper both talk to these contracts. Used by trading tools and integrations.
- `$HOME/ki/sai-keeper`: Defines the "sai-keeper" GraphQL interface for Sai.
- `$HOME/ki/nibi-chain/sai-trading`: **Sai: Trading bot automation service**
- `$HOME/ki/sai-docs`: Public-facing user documentation for Sai. Great conceptual
  resource.

### Infrastructure (IaC)
- `$HOME/ki/nibi-iac`: **Infrastructure as code (Terraform)**. Source of truth for live deployments,
  DNS, and endpoints. Provisions GKE clusters, RPC/archive endpoints, indexer (Heart Monitor) GraphQL,
  sai-keeper GraphQL/REST, monitoring (Grafana, PagerDuty). For detailed repo connections and
  how nibi-iac deploys other services, see [REFERENCE.md](./REFERENCE.md). | `terraform/` (environments:
  `dev-gcp`, `itn2-gcp`, `mainnet-gcp`, `nibiru-gcp`), `terraform/_modules/` (e.g. `sai-keeper`,
  `go-hm-graphql`, `heart-monitor`)

### Personal, Blog, Boku Workspace
- `$HOME/ki/boku`: **Personal knowledge base and project hub.** Monorepo for sandboxing, backups, and external brain. Use when working on epics, specs, Nibiru notes, focustime, or scripts that span nibi-chain, sai-perps, sai-website. Contains: `epics/`, `nibi/` (addr-book, cook), `free/` (todos, journal), `sde/`, `jiyuu/` (focustime, mdtoc), `scripts/`. Bun/TS + Go + Rust. Run `just test`, `just install`.
- `$HOME/ki/gh-io-ud`: **Personal site (GitHub Pages).** Astro 4 + Tailwind. Blog
  content in `src/content/post/` (schema in `src/content/config.ts`). Site config
  `src/config.yaml`, nav `src/navigation.js`. Run `just dev`, build `just b`,
  deploy `just deploy`.
