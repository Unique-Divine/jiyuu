# `nibid` Reference Notes

## EVM via `nibid`

Use these commands when the user wants EVM-related state through the Nibiru CLI
rather than Ethereum JSON-RPC.

Start on the intended network first:

```bash
ud nibi cfg prod
nibid config
```

Assume the active CLI config is already set to JSON output unless the user says
otherwise.

### `nibid q evm account`

This is the fastest CLI check for an EVM account's native gas balance on Nibiru.
It accepts either a hex `0x...` address or a Bech32 `nibi1...` address and
returns both forms in the response.

```bash
ADDR_HEX="0x7027d536380F0474B7Ec119c2c5250821902e6dC"
ADDR_BECH32="nibi1wqna2d3cpuz8fdlvzxwzc5jssgvs9ekufm3uj2"
nibid q evm account "$ADDR_HEX"
nibid q evm account "$ADDR_BECH32"
```

Returned fields:

- `balance_wei`: native EVM balance in wei
- `nonce`: account nonce
- `code_hash`: code hash at the address
- `eth_address`: normalized hex form
- `bech32_address`: normalized Bech32 form

Reminder:

- The empty-account code hash appears as `0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470`; seeing that value is a quick hint that the address is not holding deployed EVM bytecode.

Offline normalization only:

```bash
ADDR="0x7027d536380F0474B7Ec119c2c5250821902e6dC"
nibid q evm account "$ADDR" --offline
```

With `--offline`, the CLI only normalizes the address forms. It does not query
live chain state, so `balance_wei` and `code_hash` come back empty and `nonce`
defaults to `0`.

### `balance_wei` and bank `unibi`

`balance_wei` from `nibid q evm account` and bank `unibi` from `nibid q bank balances`
represent the same native NIBI balance, just at different granularities.

- `unibi` means micro NIBI (`μ NIBI`)
- `1 NIBI = 1_000_000 unibi`
- `1 NIBI = 10^18 wei`
- Therefore `1 unibi = 10^12 wei`

Examples:

```bash
ADDR="nibi1wqna2d3cpuz8fdlvzxwzc5jssgvs9ekufm3uj2"
DENOM="unibi"
nibid q evm account "$ADDR"
nibid q bank balances "$ADDR" --denom "$DENOM"
```

Rule of thumb:

- Use `q evm account` when the user thinks in EVM/native-gas terms or has a `0x...` address.
- Use `q bank balances --denom unibi` when the user wants the Cosmos bank representation.

### `nibid q evm funtoken`

Use this to query the mapping between a bank denom and its ERC20 representation.

```bash
COIN_OR_ERC20="unibi"
nibid q evm funtoken "$COIN_OR_ERC20"

COIN_OR_ERC20="0x7D4B7B8CA7E1a24928Bb96D59249c7a5bd1DfBe6"
nibid q evm funtoken "$COIN_OR_ERC20"
```

## `nibid tx evm` scope

On this CLI build, `nibid tx evm` is for FunToken and bank/ERC20 conversion
flows, not generic Ethereum wallet-like transactions.

Available commands:

- `nibid tx evm convert-coin-to-evm`
- `nibid tx evm convert-evm-to-coin`
- `nibid tx evm create-funtoken`

Use other surfaces when appropriate:

- `nibid tx bank` for native bank sends
- `nibid tx wasm` for CosmWasm contract execution and instantiation
- `evm-rpc` for receipts, traces, and `eth_*` / `debug_*` RPC workflows
