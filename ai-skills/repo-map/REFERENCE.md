# Repo Map Reference — Connections and IaC

This document provides deeper context on how the Nibiru repositories connect, specifically focusing on how `nibi-iac` (Infrastructure as Code) manages the deployment of application services.

## Architecture and Deployment Flow

The `nibi-iac` repository sits at the deployment layer. It consumes the artifacts or logic from application repositories and provisions the live cloud infrastructure (GCP), DNS, and endpoints.

## Key Service Connections

| App Repo | iac Module | Description |
|----------|------------|-------------|
| **nibi-go-hm** | `go-hm-graphql`, `heart-monitor` | Deploys the Heart Monitor indexer and its GraphQL interface. Defines `domain` for public access. |
| **sai-keeper** | `sai-keeper` | Deploys the Sai Keeper GraphQL/REST API. Manages `domain_graphql` and `domain_rest`. |
| **sai-trading-bot** | `sai-trading-bot`, `sai-trading-evm` | Deploys automation services for Sai exchange trading. |
| **nibi-chain** | `validator`, `fullnode`, `cosmopilot` | Provisions various blockchain node types and their Kubernetes management. |
| **nibi-installer** | `nibiru-installer` | Deploys the service behind `get.nibiru.fi`. |
| **networks-info** | `networks-info` | Deploys the service behind `networks.nibiru.fi`. |

### Supplementary Modules
Other key modules in `terraform/_modules/`:
- **Core**: `cloudsql` (PostgreSQL), `vpn-gateway`, `vault-cluster-gke`.
- **DApps/Helpers**: `faucet`, `pricefeeder`, `relayer`, `passkey-bundler`, `dextools`.
- **Operations**: `tenderduty`, `status-page`, `ping-pub`.

## Important Infrastructure Paths in `nibi-iac`

- **DNS Configuration**:
  - Mainnet/Shared: `terraform/nibiru-gcp/02-dns.tf` (Zones: `nibiru.fi`, `nibiru.org`)
  - Environment-specific (e.g. `*.internal.main.nibiru.fi`): `terraform/<env>-gcp/02-dns.tf`
- **Environment Entry Points**:
  - `terraform/mainnet-gcp/`: Production environment
  - `terraform/itn2-gcp/`: Public testnet
  - `terraform/dev-gcp/`: Development and internal testnets
- **Reusable Modules**:
  - `terraform/_modules/`: Contains the logic for deploying each component (Sai, Indexer, Nodes).

## Common Domain and Endpoint Patterns

The following patterns describe how domains are structured across environments and services:

### Public Domains
Defined in `terraform/nibiru-gcp/02-dns.tf`. CNAMEs typically point to environment-specific ingresses.

- **Mainnet**: `*.nibiru.fi` (e.g., `main.nibiru.fi`, `rpc.nibiru.fi`, `hm-graphql.nibiru.fi`, `sai-keeper.nibiru.fi`, `sai-api.nibiru.fi`).
- **Mainnet Internal**: `*.internal.main.nibiru.fi` (Grafana, Prometheus, node internal endpoints).
- **Testnet (itn2)**: `testnet-2.nibiru.fi`, `hm-graphql.testnet-2.nibiru.fi`, `sai-keeper.testnet-2.nibiru.fi`, `*.internal.test.nibiru.fi`.
- **Dev**: `devnet-1.nibiru.fi`, `*.internal.dev.nibiru.fi`, `networks.devnet.nibiru.fi`.

### Private DNS (.nibiru.local)
Accessible via VPN or Tailscale:
- **Mainnet**: `*.main.nibiru.local`
- **Testnet**: `*.testnet.nibiru.local`
- **Dev**: `*.dev.nibiru.local`

### Key Service Domains and Repos
- `hm-graphql.*.nibiru.fi` — Heart Monitor indexer GraphQL (`nibi-go-hm`)
- `sai-keeper.*.nibiru.fi`, `sai-api.*.nibiru.fi` — Sai Keeper (`sai-keeper`)
- `networks.*.nibiru.fi` — `networks-info` helper
- `get.nibiru.fi` — `nibiru-installer`
- `grafana.internal.*.nibiru.fi`, `prometheus.internal.*.nibiru.fi` — Internal monitoring

## Environment Layout and Locations

Infrastructure is split between a shared hub and environment-specific spokes:

### `nibiru-gcp` (Shared Services)
The "hub" environment that hosts common infrastructure used across all chain projects:
- **Core DNS**: Managed zones for `nibiru.fi`, `nibiru.org`, `nibiscan.io`. Entry: `terraform/nibiru-gcp/02-dns.tf`.
- **Shared Tools**: Terraform Cloud agent, SonarQube, VPN/Tailscale gateway, common observability resources.
- **Note**: This environment **does not** host chain workloads (nodes, indexers).

### Chain Environments (`mainnet-gcp`, `itn2-gcp`, `dev-gcp`)
These spoke environments host chain workloads, indexers, and application-specific logic:
- **`mainnet-gcp`**: Production mainnet chain and services (e.g., Cataclysm). Local domain: `main.nibiru.local`.
- **`itn2-gcp`**: Public testnet chain and services (e.g., testnet-2). Local domain: `testnet.nibiru.local`.
- **`dev-gcp`**: Internal devnets and early-stage testnets (e.g., devnet-1). Local domain: `dev.nibiru.local`.

Each spokes is peered with the `nibiru-gcp` hub but remains isolated from other chain projects.

## Grafana Dashboards

Dashboards are version-controlled in this repository to ensure disaster recovery and consistency across environments:

- **Mainnet**: 
  - General: `terraform/mainnet-gcp/grafana-resources/grafana_dashboards/`
  - Sai Specific: `terraform/mainnet-gcp/grafana-resources/grafana_dashboards/sai/` (e.g., perp, slp, reserves, volume, liquidations).
- **Testnet**: `terraform/itn2-gcp/grafana-resources/grafana_dashboards/`
- **Dev**: `terraform/dev-gcp/grafana-resources/grafana_dashboards/`

**Grafana UI Access (VPN Required)**:
- Mainnet: `grafana.internal.main.nibiru.fi`
- Testnet: `grafana.internal.test.nibiru.fi`
- Dev: `grafana.internal.dev.nibiru.fi`

## When to route to `nibi-iac`
- Investigating live endpoint URLs or DNS record settings.
- Changing GKE cluster configurations or resource limits.
- Updating Grafana dashboards or PagerDuty alert policies.
- Mapping how a new application service should be wired into the existing infrastructure.

## Deployment Workflow

- **Terraform Cloud**: State management and operations are centralized in Terraform Cloud. Merges to the `main` branch trigger automatic `terraform apply`. Pull requests automatically generate a `terraform plan` viewable in the Terraform Cloud workspace.
- **Pre-Commit Hooks**: The repository uses `pre-commit` to run `terraform-docs`, `tflint`, security checks (`tfsec`, `checkov`), and cost estimation (`infracost`) before each commit.
- **Modules**: Reusable modules in `terraform/_modules/` are published to Terraform Cloud and version-tagged for controlled reuse.

