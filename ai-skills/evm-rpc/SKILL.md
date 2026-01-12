---
name: evm-rpc
description: Query EVM JSON-RPC methods for transaction debugging and endpoint
  triage, including eth_getTransactionReceipt and debug_traceTransaction. Use
  when the user asks to inspect tx status, retrieve receipts, run tracers,
  compare EVM vs Cosmos outcomes, or choose between EVM RPC and Comet RPC
  endpoints.
---

# EVM RPC: Receipt and Trace Playbook

Use this skill for EVM transaction debugging with JSON-RPC.

## Quick Start

Set a hash and endpoint, then run receipt first and trace second.

```bash
evm_tx="0xYOUR_evm_tx"
EVM_RPC="https://evm-rpc.archive.testnet-2.nibiru.fi"

curl -s -X POST \
  -H "Content-Type: application/json" \
  --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getTransactionReceipt\",\"params\":[\"$evm_tx\"],\"id\":1}" \
  "$EVM_RPC" | jq

curl -s -X POST \
  -H "Content-Type: application/json" \
  --data "{\"jsonrpc\":\"2.0\",\"method\":\"debug_traceTransaction\",\"params\":[\"$evm_tx\",{\"tracer\":\"callTracer\"}],\"id\":1}" \
  "$EVM_RPC" | jq
```

## Endpoint Map - Nibiru Examples

Chain-agnostic rule: methods determine endpoint family.

| Purpose | Example endpoint | Methods |
|---|---|---|
| EVM JSON-RPC (mainnet) | `https://evm-rpc.nibiru.fi` | `eth_*`, `debug_*`, `net_*`, `web3_*` |
| EVM JSON-RPC archive (mainnet) | `https://evm-rpc.archive.nibiru.fi` | historical reads, tracing |
| EVM JSON-RPC archive (testnet) | `https://evm-rpc.archive.testnet-2.nibiru.fi` | testnet tracing and historical reads |
| Comet RPC archive (mainnet) | `https://rpc.archive.nibiru.fi` | `/block_results`, `/consensus_params` |
| Comet RPC archive (testnet) | `https://rpc.archive.testnet-2.nibiru.fi` | `/block_results`, `/consensus_params` |

Rules:
- Run `eth_*` and `debug_*` only on EVM RPC endpoints.
- Run `/block_results` and `/consensus_params` only on Comet RPC endpoints.
- Prefer archive endpoints for old heights/hashes and tracing workflows.

## Basic Queries (Health / Identity)

### web3_clientVersion

Queries the traceability info like the client name, version, Git commit, Go version,
runtime architecture, and build tags.

```bash
curl -X POST https://evm-rpc.nibiru.fi \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "web3_clientVersion",
    "params": [],
    "id": 1
  }'
```

The response looks like this: "Nibiru 2.6.0: Compiled at Git commit 3cf97e3468f8ce922c8760f839f8f336ca0c37f4 using Go go1.24.5, arch amd64, and build tags (netgo osusergo ledger static rocksdb pebbledb muslc)"

### eth_chainId

Queries the EIP-155 replay-protection chain id for the current ethereum blockchain (Nibiru).

```bash
curl -X POST https://evm-rpc.nibiru.fi \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "eth_chainId",
    "params": [],
    "id": 1
  }'
```

## Transaction Triage Workflow

Use this sequence to avoid false conclusions:

1. `eth_getTransactionByHash`
   - Confirms the tx exists and gives gas fields, type, input, and block refs.
2. `eth_getTransactionReceipt`
   - Authoritative EVM execution result.
3. `debug_traceTransaction` (usually `callTracer`)
   - Explains internal call path and where execution diverged.
4. Optional cross-layer checks on Comet RPC
   - `block_results?height=...`, `consensus_params?height=...`.

## Concrete Command Cookbook

### 1 - Get Transaction by Hash

```bash
evm_tx="0xYOUR_evm_tx"
EVM_RPC="https://evm-rpc.archive.testnet-2.nibiru.fi"

curl -s -X POST \
  -H "Content-Type: application/json" \
  --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getTransactionByHash\",\"params\":[\"$evm_tx\"],\"id\":1}" \
  "$EVM_RPC" | jq
```

### 2 - Get Transaction Receipt - Authoritative Status

```bash
evm_tx="0xYOUR_evm_tx"
EVM_RPC="https://evm-rpc.archive.testnet-2.nibiru.fi"

curl -s -X POST \
  -H "Content-Type: application/json" \
  --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getTransactionReceipt\",\"params\":[\"$evm_tx\"],\"id\":1}" \
  "$EVM_RPC" | jq
```

Interpretation:
- `result == null`: **Not mined** or rejected. Either the tx is still in the mempool, it was dropped, or it failed consensus/validation (e.g., bad nonce, invalid signature, insufficient funds for gas). It is not in any block.
- `status == "0x1"`: **Full Success**. The transaction was executed successfully, and all state changes were applied.
- `status == "0x0"`: **Execution Failure (Revert)**. The transaction was successfully mined and included in a block (consuming gas), but the EVM execution was reverted. No state changes occurred except for the gas fee deduction from the sender.
- `blockNumber` present: Mined/included in a block.
- `logs` present: Events emitted (decode by `address`, `topics`, `data`). Note that a reverted tx (`0x0`) will generally have an empty `logs` array, even if it attempted to emit them before reverting.

### 3 - Run Debug Tracer - Call Tree View

```bash
evm_tx="0xYOUR_evm_tx"
EVM_RPC="https://evm-rpc.archive.testnet-2.nibiru.fi"

curl -s -X POST \
  -H "Content-Type: application/json" \
  --data "{\"jsonrpc\":\"2.0\",\"method\":\"debug_traceTransaction\",\"params\":[\"$evm_tx\",{\"tracer\":\"callTracer\"}],\"id\":1}" \
  "$EVM_RPC" | jq
```

Interpretation:
- Inspect top-level `from`, `to`, `gas`, `gasUsed`.
- Walk `calls[]` for internal call chain and outputs.
- Use trace to explain behavior; use receipt status to decide success/failure.

### 4 - Cross-Layer Checks with Comet RPC

```bash
HEIGHT="5959008"
TM_RPC="https://rpc.archive.testnet-2.nibiru.fi"

curl -s "$TM_RPC/block_results?height=$HEIGHT" | jq
curl -s "$TM_RPC/consensus_params?height=$HEIGHT" | jq
```

Use when EVM and Cosmos observations appear inconsistent.

## Receipt and Trace Rules of Thumb

- **Receipt status is the authoritative EVM truth** for success vs. failure.
- **Mined does not mean successful**: A transaction can be in a block and consume gas (`0x0`), but have zero effect on state. Always check the `status` field.
- **Null receipt implies non-existence**: If `eth_getTransactionReceipt` returns `null`, the transaction was never mined. This is usually due to a consensus/mempool error (invalid signature, wrong chain ID, bad nonce, or insufficient funds to cover the max gas cost).
- **Consensus failures don't have receipts**: Errors like "insufficient funds" or "bad nonce" prevent a transaction from even entering a block, so no EVM status is ever generated.
- **A non-empty `logs` array** (at the receipt level) generally only exists for successful transactions (`0x1`).
- **Gas consumption**: Even in failure (`0x0`), the `gasUsed` is still charged to the sender up to the point of reversion.
- **Gas Comparison**: Compare receipt `gasUsed` with the `gas` (limit) from `eth_getTransactionByHash` to see if the transaction ran out of gas.
- **Tracing Failures**: If tracing fails, verify the provider supports the `debug` namespace and retry on an archive endpoint.

## Endpoint Selection Guide

Choose endpoint by method and historical depth:

- Latest, simple reads (`eth_blockNumber`, recent receipt):
  - Standard EVM RPC is usually enough.
- Historical receipts, old block state, trace/debug:
  - Prefer archive EVM RPC.
- Tendermint/Comet block metadata and app-level event correlation:
  - Use Comet RPC endpoints.

## Common Pitfalls

- Calling Comet paths on EVM RPC host (or the reverse).
- Using the wrong network endpoint for a valid tx hash.
- Assuming `result == null` always means pending; it may be wrong chain/provider.
- Treating trace output alone as final status without reading receipt.
- Ignoring timeout/retry behavior for freshly broadcast transactions.

## Reusable Triage Template

```bash
evm_tx="0xYOUR_evm_tx"
EVM_RPC="https://evm-rpc.archive.testnet-2.nibiru.fi"
TM_RPC="https://rpc.archive.testnet-2.nibiru.fi"

# 1) tx details
curl -s -X POST -H "Content-Type: application/json" \
  --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getTransactionByHash\",\"params\":[\"$evm_tx\"],\"id\":1}" \
  "$EVM_RPC" | jq

# 2) receipt status
curl -s -X POST -H "Content-Type: application/json" \
  --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getTransactionReceipt\",\"params\":[\"$evm_tx\"],\"id\":1}" \
  "$EVM_RPC" | jq

# 3) call trace
curl -s -X POST -H "Content-Type: application/json" \
  --data "{\"jsonrpc\":\"2.0\",\"method\":\"debug_traceTransaction\",\"params\":[\"$evm_tx\",{\"tracer\":\"callTracer\"}],\"id\":1}" \
  "$EVM_RPC" | jq
```

## Additional Resources

- Zero-gas debug walkthrough:
  `/home/realu/ki/boku/epics/epic-evm/26-02-zero-gas/26-02-10-zero-gas-debug.md`
- Existing endpoint conventions:
  `/home/realu/ki/boku/nibi/cook/index.ts`
