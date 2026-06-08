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

### `nibid q evm balance`

This is the workhorse command for checking USDC balances on Nibiru, especially
for Sai operations where USDC is the primary currency. It queries token balances
in Nibiru's multi-VM context and returns both ERC20 and Bank representations
when available.

It accepts either address format for the account argument:

- EVM hex address, e.g. `0x8fB11A655B6cb989085c8B21AB044Ec19685942D`
- Bech32 address, e.g. `nibi137c35e2mdjucjzzu3vs6kpzwcxtgt9pdnvj5ll`

It accepts either token format for the token argument:

- ERC20 address, e.g. mainnet USDC `0x0829F361A05D993d5CEb035cA6DF3446b060970b`
- Bank denom, e.g. `erc20/0x0829F361A05D993d5CEb035cA6DF3446b060970b`
- Native bank denom, e.g. `unibi`

Mainnet Sai USDC reference:

```bash
USDC_ERC20="0x0829F361A05D993d5CEb035cA6DF3446b060970b"
USDC_BANK="erc20/0x0829F361A05D993d5CEb035cA6DF3446b060970b"
```

The examples from `nibid q evm balance --help` are the canonical quick-start:

```bash
# USDC on Nibiru mainnet
addr="0xYourAddr" token="0x0829F361A05D993d5CEb035cA6DF3446b060970b"
nibid query evm balance "$addr" "$token"

# NIBI via bank denom
addr="0xYourAddr" token="unibi"  # Bank coin denom works
nibid query evm balance "$addr" "$token"

# NIBI via canonical WNIBI ERC20
addr="0xYourAddr" token="0x0CaCF669f8446BeCA826913a3c6B96aCD4b02a97" # ERC20 addr works
nibid query evm balance "$addr" "$token"
```

For a Bech32 keyring address, use the key address directly:

```bash
ADDR="$(nibid keys show sai-perp-admin -a)"
TOKEN="0x0829F361A05D993d5CEb035cA6DF3446b060970b"
nibid q evm balance "$ADDR" "$TOKEN" | jq .
```

Useful output fields:

- `addr_evm` and `addr_bech32`: normalized account address forms.
- `erc20_balance_human` / `erc20_balance_base`: ERC20 balance.
- `bank_balance_human` / `bank_balance_base`: Bank coin balance for the mapped
  denom.
- `bank_coin_denom`: bank denom corresponding to the ERC20 token.
- `erc20_decimals` / `bank_decimals`: decimal metadata for display conversion.

Rule of thumb:

- Prefer `nibid q evm balance "$ADDR" "$USDC_ERC20"` for Sai USDC balance
  checks because it shows both EVM/ERC20 and Bank-side holdings in one response.
- Use `nibid q bank balances "$ADDR" --denom "$USDC_BANK"` only when you
  specifically need the Bank module view.

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
