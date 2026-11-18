---
name: sai
description: >-
  Trade and inspect Sai perpetuals on Nibiru with the open-source `sai` CLI
  and its MCP server, and query or reconcile Sai Perps data from live
  contracts, Sai Keeper GraphQL, and DexPal REST. Use for Sai markets, prices,
  positions, balances, trade history, live watches, connecting a Sai account,
  previewing or submitting trades, checking intents, installing or
  configuring the Sai MCP server, `sai config`, `sai doctor`, `sai update`,
  fees, open interest, SLP vaults, referrals, or disagreements between
  on-chain and indexed data. Use agent skill `sai-ops` for contract and
  operator mutations.
---

# Sai

Sai is a perpetual futures exchange on Nibiru. Traders deposit USDC, and Sai
Keeper turns each signed trading request, called an intent, into a contract
transaction. The `sai` CLI from
[SaiDotFun/open-sai](https://github.com/SaiDotFun/open-sai) reads markets and
accounts, streams updates, submits intents, and runs an MCP server with the
same reads and trades.

This skill covers two kinds of work: operating Sai through the CLI or MCP
tools, and reading protocol data directly from contracts and APIs. Read only
the pages the task needs.

## Install

Check for the CLI with `sai --version`. If it is missing, install it with
[uv](https://docs.astral.sh/uv/):

```bash
uv tool install git+https://github.com/SaiDotFun/open-sai
```

To try it without installing:

```bash
uvx --from git+https://github.com/SaiDotFun/open-sai sai --json market prices
```

The package is `sai-cli`. `uv tool install sai` installs an unrelated PyPI
package.

## Which Page to Read

| Task | Page |
| --- | --- |
| Run `sai` commands: markets, accounts, watches, connecting an account, settings, `sai doctor`, `sai update` | [cli.md](cli.md) |
| Register the MCP server with an agent client, or use Sai MCP tools | [mcp.md](mcp.md) |
| Anything that previews, submits, or follows a trade, through the CLI or MCP | [trading-safety.md](trading-safety.md) |
| Live Perp and Oracle contract state: markets, collaterals, OI limits, fees, deposits, credits | [references/contracts.md](references/contracts.md) |
| SLP Vault health, share price, deposit caps, epochs, raw vault state | [references/vaults.md](references/vaults.md) |
| Sai Keeper GraphQL: indexed trades, LP history, prices, referrals, subscriptions | [references/graphql.md](references/graphql.md) |
| DexPal REST: public stats, markets, yield, health checks | [references/rest.md](references/rest.md) |
| Referral code ownership, redemptions, testnet balance and credit checks | [references/referrals.md](references/referrals.md) |
| Why contract state, GraphQL, and REST disagree | [references/reconciliation.md](references/reconciliation.md) |

Read [trading-safety.md](trading-safety.md) before the first trade of a
session, even when the person asks for one quick order.

## Rules for Every Task

- If Sai MCP tools such as `mcp_info` and `market_prices` are available,
  prefer them over shell commands.
- Command `sai --help` and each command's `--help` are the authority for
  options. Put `--json` before the command for machine-readable output:
  `sai --json market list`.
- Mainnet is the default network.
- Never ask the person for a session token, and never type, paste, or echo
  one. The person signs in with `sai auth import` or `sai auth login`.
- Say which account a trade uses before the person confirms it.
- Keeper accepting an intent does not mean the trade executed.

## Copies of This Skill

This skill is edited in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/tree/main/ai-skills/sai)
and packaged with the CLI. Command `sai skill show`, or `sai --skill` on
older versions, prints the packaged copy, and `sai skill source` names the
commit it was copied from. When this page and the installed CLI disagree
about a command, follow `sai --help`.

## Outside This Skill

- Contract uploads, migrations, admin messages, creating referral codes,
  issuing trading credits, and other operator changes belong to agent skill
  `sai-ops`.
- The Sai Keeper Postgres schema and direct database access belong to private
  agent skill `sai-db`. This skill needs no database access or credentials.
- Heart Monitor GraphQL for chain transactions and Wasm events belongs to
  agent skill `nibiru-gql`. `nibiru` CLI mechanics belong to agent skill
  `nibiru-cli`.

## Reading Protocol Data

For a question that spans several sources:

1. Identify the network and the entity: contract address, market index, token
   index, trader, vault, referral code, REST endpoint, or GraphQL field.
2. Read live contract state when the fact controls protocol behavior.
3. Read GraphQL or REST when the fact is about what the app or API shows.
4. Compare sources explicitly. Say which one owns each fact and which is
   stale, derived, or human-maintained.

Common mistakes:

- Contract query JSON uses CosmWasm `snake_case`. GraphQL fields use
  camelCase.
- On-chain amounts and GraphQL `Int` amounts are base units. REST responses
  often return human or USD values.
- `TokenIndex(0)` is the USD quote placeholder, not collateral. Mainnet
  collateral is `TokenIndex(1)` for USDC and `TokenIndex(2)` for stNIBI.
- Some contract queries take wrapped string indices such as
  `"MarketIndex(0)"`, and others take plain integers. Check
  [references/contracts.md](references/contracts.md) before guessing.
- REST 24-hour windows differ. `GET /dexpal/v1/stats` and
  `GET /dexpal/v1/markets/details` can use different time anchors.

[examples/prompts.md](examples/prompts.md) lists protocol-data prompts that
should and should not use this skill.
