---
name: nibiru-cli-nibid
description: Query Nibiru chain state and submit transactions with the `nibid` CLI, including setup via `ud`, config inspection, key discovery with `nibid keys list`, bank transfers, bank balance queries, wasm contract queries, wasm execute/instantiate flows, and transaction lookups by hash. Use when the user mentions `nibid`, Nibiru CLI, `nibid keys list`, `--from`, `nibid tx bank`, `nibid tx wasm`, `nibid q bank`, `nibid q wasm`, `nibid q tx`, balances, contract-state smart, or tx hashes.
---

# Nibiru CLI (`nibid`)

Use this skill for general Nibiru CLI work. Prefer it when the user wants the
authoritative chain view or wants to submit a transaction with `nibid`.

## Additional resources

- EVM account, FunToken, and unit notes: [`reference.md`](./reference.md)
- Copy-paste examples: [`examples.md`](./examples.md)

## Quick start

1. Select the chain config with your `ud` wrapper:

```bash
ud nibi cfg prod
```

2. Confirm what `nibid` is pointed at:

```bash
nibid config
```

Example - Mainnet.
```js
{
	"chain-id": "cataclysm-1",
	"keyring-backend": "test",
	"output": "json",
	"node": "https://rpc.archive.nibiru.fi:443",
	"broadcast-mode": "sync"
}
```

3. Assume JSON output by default. 
All default `nibid` configs use `nibid config output json`, so commands inherit
JSON output unless the user says otherwise.

```bash
ADDR="nibi1..."
nibid q bank balances "$ADDR"
```

4. Discover which local keys are available for `--from`:

```bash
nibid keys list | jq

# names that are usually valid for direct signing
nibid keys list | jq -r '.[] | select(.type=="local") | .name'
```

Rules of thumb:

- `type == "local"` is the default set of names to use for direct `--from`
  signing.
- `type == "offline"` is useful for address/pubkey reference, not direct local
  signing.
- `type == "multi"` is a multisig identity and usually needs a multisig flow,
  not a simple single-signer `--from`.
- If the user says "use the `validator-6900` key", prefer `--from
  validator-6900` when that name appears in `nibid keys list`.

## Core mental model

- `nibid q ...` / `nibid query ...` reads chain state.
- `nibid tx ...` builds and broadcasts transactions.
- `nibid config` shows the current CLI configuration and is the first check when
  a result looks wrong for the expected network.
- Use `ud nibi cfg ...` to switch networks before querying or sending.
- Use `nibid keys list | jq` to discover available `--from` values before
  proposing or running a transaction.
- Pipe to `jq` for nested fields when needed.
- Use specialized skills for deeper workflows:
  - `nibid-gov-upgrade` for governance and software upgrades
  - `sai-perps-query` for Sai contract query recipes
  - `evm-rpc` for `eth_*` / `debug_*` EVM tracing rather than Cosmos CLI

## Query routing

If the user asks about:

- **Wallet balances** -> use `nibid q bank ...`
- **Wasm contract state** -> use `nibid q wasm ...`
- **A tx hash** -> use `nibid q tx "$TX"`
- **Sending funds** -> use `nibid tx bank ...`
- **Executing or instantiating a contract** -> use `nibid tx wasm ...`

## Golden paths

### Query Bank

```bash
# all balances for an account
ADDR="nibi1..."
nibid q bank balances "$ADDR"

# one denom balance
DENOM="unibi"
nibid q bank balances "$ADDR" --denom "$DENOM"

# total supply for a denom
nibid q bank total "$DENOM"
```

Notes:

- Amounts are usually base units such as `unibi`.
- For display, divide `unibi` by `1e6` to get `NIBI`.

### Tx Bank

```bash
# send tokens
FROM_KEY="<from-key-or-addr>"
TO_ADDR="nibi1recipient..."
AMOUNT="1000000unibi"
nibid tx bank send "$FROM_KEY" "$TO_ADDR" "$AMOUNT"
```

Common flags to add when needed:

```bash
--from <key-or-addr> --gas auto --gas-adjustment 1.3 --fees 10000unibi
```

Rule of thumb:

- Use `tx bank send` for simple transfers.
- Prefer a key name from `nibid keys list` for `--from`, for example `--from
  validator-6900`.
- Use `--note` when the receiving flow needs a memo or exchange-style reference.
- Confirm the active network with `nibid config` before sending anything.
- See [`examples.md`](./examples.md) for a copy-paste bank send with memo.

### Query Wasm 

```bash
# contract metadata
CONTRACT_ADDR="nibi1..."
nibid q wasm contract "$CONTRACT_ADDR"

# list contracts by code id
CODE_ID="123"
nibid q wasm list-contract-by-code "$CODE_ID"

# raw smart query
QUERY_MSG='{"get_config":{}}'
nibid q wasm contract-state smart "$CONTRACT_ADDR" "$QUERY_MSG"
```

Rules of thumb:

- `contract-state smart` is the main path for app-level contract queries.
- Query JSON keys are contract-specific and usually `snake_case`.
- Reuse a known-good query message shape from the relevant repo or skill when
  the exact message is not obvious.

#### Discover query variants without source

If you do not have the contract source or schema, probe with an unknown query
key. Many CosmWasm contracts return a serde enum error listing the accepted
query enum variants.

```bash
CONTRACT_ADDR="nibi1..."
nibid q wasm contract-state smart "$CONTRACT_ADDR" \
  '{"_key_probe":{}}'
```

Example error:

```text
unknown variant `_key_probe`, expected one of `config`, `state`, ...
```

This is a discovery hint, not a full schema; enum variants may still need
required fields.

### Tx Wasm

```bash
# execute a contract
CONTRACT_ADDR="nibi1..."
EXECUTE_MSG='{"claim":{}}'
FROM_KEY="<key-or-addr>"
nibid tx wasm execute "$CONTRACT_ADDR" "$EXECUTE_MSG" --from "$FROM_KEY"

# instantiate a contract from a code id
CODE_ID="123"
INIT_MSG='{"count":0}'
LABEL="my-contract"
nibid tx wasm instantiate "$CODE_ID" "$INIT_MSG" --label "$LABEL" --from "$FROM_KEY"
```

Common additions:

```bash
--admin <nibi1...> --amount 1000000unibi --gas auto --gas-adjustment 1.5 --fees 10000unibi
```

Rules of thumb:

- `execute` changes state and usually needs `--from`.
- Prefer a key name from `nibid keys list` for `--from`, not a guessed alias.
- `instantiate` creates a new contract instance from uploaded code.
- If funds must be sent alongside execution or instantiation, use `--amount`.
- Keep the JSON message exact; contract query/execute schemas are not inferred by
  the CLI.

### Transaction lookup by hash

```bash
TX="ABCD1234..."
nibid q tx "$TX"
```

Useful follow-up:

```bash
nibid q tx "$TX" | jq .
```

Rules of thumb:

- Start with `q tx` when the user gives a Cosmos-style tx hash.
- If the user gives an EVM tx hash and wants receipt/trace semantics, use the
  `evm-rpc` skill instead.

## Working conventions

1. Start by checking or setting the network. This is how you move between using
different instances of the Nibiru blockchain.

```bash
ud nibi cfg prod
nibid config
```

2. Before any signing flow, list the keyring and pick an explicit `--from` value:

```bash
nibid keys list | jq
nibid keys list | jq -r '.[] | select(.type=="local") | .name'
```

  - This is useful for debugging, as it can bring to light a missing signer or be
  used to find potential accounts with funds under management.

3. When command output is too broad, extract the exact field with `jq`.
4. If the user asks for explorer-style views, derive them from CLI JSON rather
  than treating the explorer as the source of truth.

## Common pitfalls

- Wrong network selected in the CLI config.
- Guessing a `--from` value instead of checking `nibid keys list`.
- Using human units instead of base units like `unibi`.
- Treating EVM JSON-RPC questions as `nibid` questions.
- Guessing Wasm query message shapes instead of checking the contract or app code.