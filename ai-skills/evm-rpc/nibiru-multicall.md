# Batched Reads on Nibiru with Multicall3 and Precompiles

One `eth_call` can read ERC20 balances, bank balances, native NIBI, and Wasm
contract state for many addresses at once. Use it instead of one request per
address or per token.

## How It Works

Multicall3 is deployed on Nibiru mainnet at the usual address,
`0xcA11bde05977b3631167028862bE2a173976CA11`. Its
`aggregate3((address target, bool allowFailure, bytes callData)[])` runs each
entry as a call inside one EVM execution and returns
`(bool success, bytes returnData)[]`. With `allowFailure: true`, a reverting
entry reports `success = false` without failing the batch. Sent through
`eth_call`, the batch is read-only and costs nothing.

On most chains that only reaches EVM state. Nibiru's precompiles answer with
Cosmos module state, and Multicall3 can call them like any contract:

| Address | Precompile | View calls |
| --- | --- | --- |
| `0x0000000000000000000000000000000000000800` | FunToken | `bankBalance(address who, string denom)` returns `(uint256, (address ethAddr, string bech32Addr))`; `balance(address who, address funtoken)` returns ERC20 and bank balances of a FunToken; `whoAmI(string)`; `getErc20Address(string denom)` |
| `0x0000000000000000000000000000000000000801` | Oracle | `queryExchangeRate(string pair)`, Chainlink-style `latestRoundData` |
| `0x0000000000000000000000000000000000000802` | Wasm | `query(string contract, bytes json)` returns the JSON response as bytes; `queryRaw(string contract, bytes key)` |

Interfaces: nibi-chain `evm/embeds/contracts/IFunToken.sol`, `IOracle.sol`,
and `Wasm.sol`.

Native NIBI comes from Multicall3's own `getEthBalance(address)`. It is the
bank `unibi` balance shown with 18 decimals instead of 6.

## Example

Four reads for one address in one request: native NIBI, a bank denom, an
ERC20 balance, and a Wasm smart query.

```bash
RPC=https://evm-rpc.nibiru.fi
MC=0xcA11bde05977b3631167028862bE2a173976CA11
WHO=0xYourAddress
TOKEN=0xSomeErc20
WASM=nibi1somecontract
c1=$(cast calldata 'getEthBalance(address)' $WHO)
c2=$(cast calldata 'bankBalance(address,string)' $WHO unibi)
c3=$(cast calldata 'balanceOf(address)' $WHO)
c4=$(cast calldata 'query(string,bytes)' $WASM $(cast from-utf8 '{"config":{}}'))
cast call $MC 'aggregate3((address,bool,bytes)[])((bool,bytes)[])' \
  "[($MC,true,$c1),(0x0000000000000000000000000000000000000800,true,$c2),\
($TOKEN,true,$c3),(0x0000000000000000000000000000000000000802,true,$c4)]" \
  --rpc-url $RPC
```

Decode each `returnData` with its own type:

```bash
cast decode-abi 'f()(uint256)' <data>                  # getEthBalance, balanceOf
cast decode-abi 'f()(uint256,(address,string))' <data> # bankBalance
cast --to-ascii $(cast decode-abi 'f()(bytes)' <data>) # Wasm query JSON
```

## Many ERC20 Reads

Repeat `(token, true, balanceOf(holder))` for each token and holder pair in
the same array. Add `symbol()` (`0x95d89b41`) and `decimals()`
(`0x313ce567`) entries per token to format the results without extra
requests.

## Limits

- `bankBalance` reads one denom per entry; there is no all-balances call.
  Use the bank `AllBalances` query to discover unknown denoms.
- Precompiles take hex addresses. A `nibi1…` address holds the same 20 bytes;
  convert it locally. One entry's output can't feed a later entry.
- Each `eth_call` runs under the node's gas cap. Split very large batches
  into several `eth_call`s and send them as one JSON-RPC batch array, still
  one HTTP POST.
- `eth_call` reads a single block. Pass a block tag and use an archive
  endpoint for past heights.
