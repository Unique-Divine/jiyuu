# Heart Monitor transaction and Wasm event verification

Use this reference to prove that Heart Monitor indexed a Cosmos transaction or
a custom CosmWasm event. This is evidence about the Nibiru chain indexer, not
about a Sai Keeper derived-state write.

## Transaction message lookup

GraphQL field `messages` returns indexed Cosmos messages. Query it with the
Cosmos transaction hash:

```graphql
query TransactionMessages($txHash: String!) {
  messages(where: { txHash: $txHash }) {
    block { block block_ts }
    txHash
    txCode
    txSeqNo
    msgSeqNo
    type
    senderAddr
    contractAddr
    action
    funds { denom amount }
  }
}
```

Interpret `txCode = 0` as a successful Cosmos transaction. For a Wasm execute,
field `action` identifies the execute-message variant, but the message record
does not include all emitted custom-event attributes.

## Custom Wasm event lookup

GraphQL field `wasm.contractEvents` returns custom event records and attributes.
Use its block and contract-address filters, then select the target transaction
hash or event type from the response:

```graphql
query ContractEvents($block: Int!, $contract: String!) {
  wasm {
    contractEvents(
      where: {
        block: { eq: $block }
        contractAddress: { eq: $contract }
      }
    ) {
      block { block block_ts }
      txSeqNo
      eventSeqNo
      txHash
      type
      contractAddress
      attributes { key value }
    }
  }
}
```

For example, a Sai credit issue should expose custom event type
`wasm-sai/perp/trading_credit` with attributes including `action=issue`, `user`,
`creds`, and `new_credit_balance`.

### Current filter caveat

The GraphQL input type `ContractEventsFilter` exposes a `type` filter, but the
mainnet resolver can return PostgreSQL error `column reference "type" is
ambiguous` when that filter is used. Query by block and contract address, then
filter the returned event list locally until that resolver is fixed.

## Incident interpretation

For a Sai deposit or trading-credit incident:

1. A Heart Monitor custom-event record proves HM indexed the chain event.
2. A matching row in Sai Keeper table `sai_user_deposit_balances` proves Keeper
   processed it into derived state.
3. A Sai Keeper GraphQL subscription response proves the API exposes that
   derived state.
4. If all three agree but the user reports a missing balance, investigate the
   connected wallet, client session, or UI rendering before opening an indexer
   incident.

Use agent skill `sai-db` for the second step and agent skill `sai-perps` for
live contract state and the full cross-source reconciliation.
