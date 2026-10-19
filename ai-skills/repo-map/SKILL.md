---
name: repo-map
description: >-
  Maps and routes between key repositories on my machine (boku, nibi-chain,
  nibi-geth, nibi-go-hm, nibi-ts-sdk, sai-web, sai-perps, nibi-iac,
  nibi-web, gh-io-ud, and the in-tree Wasm runtime and Rust contracts). Use when the user
  asks which repo to look at, where a feature lives, how repos connect, how
  services are deployed (Terraform/IaC), when a task spans chain, EVM, indexer,
  SDK, frontend, and docs, or when the user asks about the personal site, blog
  posts, official Nibiru docs, or Astro/gh-pages.
---

# Repo Map — Nibiru and Personal Repos

Use this skill to quickly route questions/tasks to the correct repo(s) under path `$HOME/ki/`.

## Quick routing

### Blockchain and EVM Impl
- `$HOME/ki/nibi-chain`: **Chain logic, modules, and Nibiru CLI (`nibiru`)**. This repo
defines the logic, and you can run `nibiru tx`, `nibiru query`, or other commands in
the terminal to inspect exact functionality. Core L1 chain (Go). App wiring +
custom modules. Includes `cmd/nibid`. | `x/`, `app/`, `proto/`, `cmd/nibid/`
- `$HOME/ki/nibi-chain/x/evm`: **Nibiru EVM execution and precompiles live here.**. RPC is standard for the EVM.
- `$HOME/ki/nibi-geth`: **EVM state transition internals that do not involve precompiles; go-ethereum logic**
- `$HOME/ki/nibi-chain/lib/wasmvm`: **Nibiru's maintained Go WasmVM bindings and Rust FFI runtime**.
The Go types live in `wvm/`, and the Rust crate lives in `libwasmvm/`.
Follow this directory's `README.md` for builds and releases.
- `$HOME/ki/nibi-chain/lib/cosmwasm-vm` and `$HOME/ki/nibi-chain/lib/wasmer`:
**Nibiru's maintained Rust VM and patched Wasmer runtime**. Dependency paths
connect these in-tree directories to `lib/wasmvm/libwasmvm`.
- `$HOME/ki/nibi-chain/wasm-contracts` and Rust packages under `$HOME/ki/nibi-chain/lib`:
**Nibiru's maintained CosmWasm contracts and reusable Rust crates**, including
`nibiru-std` and `nibiru-ownable`. Read `nibi-chain/spec/04-rust-contract-workspace.md`
for the root Cargo workspace and `just rs` commands.
- **Archived standalone forks**: `NibiruChain/go-wasmvm` and
`NibiruChain/nibiru-wasm` are public archives. Their former local checkouts
`wasm-go-wasmvm` and `nibi-wasm` were removed. Route maintained work to the
in-tree directories above; consult archived tags and releases only for history.
- `$HOME/ki/wasm-cosmwasm`: **Upstream CosmWasm reference checkout**,
`CosmWasm/cosmwasm`. Nibiru's maintained VM implementation lives in the monorepo.

### Nibiru Indexer and TypeScript SDK
- `$HOME/ki/nibi-go-hm`: 
   - **Indexer server (Heart Monitor) and GraphQL schema**. This is the Nibiru
   Indexer schema's source of truth. `graphql/graph/*.graphqls` (e.g. `staking.graphqls`, `user.graphqls`)
- `$HOME/ki/nibi-ts-sdk`: **TypeScript SDK (`@nibiruchain/nibijs`)** and Nibiru
Indexer (HeartMonitor) client. Look for `HeartMonitor`, generated `GQL*` types,
and query builders.

### Sai Exchange
- `$HOME/ki/sai-web/webapp`: **Sai: web app** (lives in monorepo). Web
frontend and API integrations. This uses the "sai-keeper" GraphQL interface.
- `$HOME/ki/sai-perps`: **Official Sai protocol contracts** (EVM + Wasm). This repo defines what gets deployed onchain; the web app and sai-keeper both talk to these contracts. Used by trading tools and integrations.
- `$HOME/ki/sai-keeper`: Defines the "sai-keeper" GraphQL interface for Sai.
- `$HOME/ki/nibi-chain/sai-trading`: **Sai: Trading bot automation service**
- `$HOME/ki/sai-docs`: Public-facing user documentation for Sai. Great conceptual
  resource.

### Nibiru public site, docs, and wallet app
- `$HOME/ki/nibi-web`: **Combined Nibiru website**, `NibiruChain/nibi-web`.
  Astro marketing and blog source lives in `src/`. VitePress docs live in
  `docs-v2/`, with content synced from `nibi-chain/docs`. React wallet app source
  lives in `app/`. One Bun workspace builds `/`, `/docs`, and `/app` into `dist`
  for a shared Firebase release. Use root `just dev`, `just build`, and
  `just preview`; focused app commands remain available.
- Legacy names `nibi-home-site`, `home-site`, and `nibi-webapp` route maintained
  work to `nibi-web`, or `nibi-web/app` for app features. Existing old worktrees
  use `nibi-web-*` names when retained for user branches and history. The main
  checkout is `$HOME/ki/nibi-web`; linked worktrees share its Git directory.
  Do not delete or repurpose worktrees with unique work.
- Archived `NibiruChain/nibi-webapp` retains historical app PR discussions and releases.
  New app PRs belong in `NibiruChain/nibi-web`.

### Infrastructure (IaC)
- `$HOME/ki/nibi-iac`: **Infrastructure as code (Terraform)**. Source of truth for live deployments,
  DNS, and endpoints. Provisions GKE clusters, RPC/archive endpoints, indexer (Heart Monitor) GraphQL,
  sai-keeper GraphQL/REST, monitoring (Grafana, PagerDuty). For detailed repo connections and
  how nibi-iac deploys other services, see [REFERENCE.md](./REFERENCE.md). | `terraform/` (environments:
  `dev-gcp`, `itn2-gcp`, `mainnet-gcp`, `nibiru-gcp`), `terraform/_modules/` (e.g. `sai-keeper`,
  `go-hm-graphql`, `heart-monitor`)

### Personal, Blog, Boku Workspace
- `$HOME/ki/boku`: **Personal knowledge base and project hub.** Monorepo for sandboxing, backups, and external brain. Use when working on epics, specs, Nibiru notes, focustime, or scripts that span nibi-chain, sai-perps, sai-web. Contains: `epics/`, `nibi/` (addr-book, cook), `free/` (todos, journal), `sde/`, `jiyuu/` (focustime, mdtoc), `scripts/`. Bun/TS + Go + Rust. Run `just test`, `just install`.
- `$HOME/ki/gh-io-ud`: **Personal site (GitHub Pages).** Astro 4 + Tailwind. Blog
  content in `src/content/post/` (schema in `src/content/config.ts`). Site config
  `src/config.yaml`, nav `src/navigation.js`. Run `just dev`, build `just b`,
  deploy `just deploy`.

## GitHub Remotes and Git Cloning

### Repos with Prefix "sai-" or "nibi-" (`$HOME/ki`)

Nibiru work lives under org **`NibiruChain`**. Local directory names often **differ** from the repository slug (e.g. clone `nibiru` into **`nibi-chain`**). **`gh repo list NibiruChain`** lists canonical slugs for your account.

| Local folder | Remote |
|--------------|--------|
| `nibi-chain` | `git@github.com:NibiruChain/nibiru.git` |
| `nibi-geth` | `git@github.com:NibiruChain/go-ethereum.git` |
| `nibi-go-hm` | `git@github.com:NibiruChain/go-heartmonitor.git` |
| `nibi-ts-sdk` | `git@github.com:NibiruChain/ts-sdk.git` |
| `nibi-web` | `git@github.com:NibiruChain/nibi-web.git` |
| `sai-web` | `git@github.com:NibiruChain/sai-web.git` |
| `sai-perps` | `git@github.com:NibiruChain/sai-perps.git` |
| `sai-keeper` | `git@github.com:NibiruChain/sai-keeper.git` |
| `sai-docs` | `git@github.com:NibiruChain/sai-docs.git` |
| `nibi-iac` | `git@github.com:NibiruChain/iac.git` |

Other entries in this map: **`wasm-cosmwasm`** → `git@github.com:CosmWasm/cosmwasm.git`; **`boku`** → `git@github.com:Unique-Divine/boku.git` (personal).
The archived Nibiru forks are `NibiruChain/go-wasmvm` and `NibiruChain/nibiru-wasm`;
cloning `NibiruChain/nibiru` provides their maintained replacements.

Copy-paste **`git clone`** commands, optional idempotent **`clone`** helper function, and notes on discovering org repos: [cloning.md](./cloning.md).
