# Sai Protocol Data Test Prompts

Use these prompts to tune whether agent skill `sai` triggers for protocol
data questions. The first group should trigger `sai` and its references; the
second group should not, or should use agent skill `sai-ops` because the task
is a private operator runbook.

## Should Trigger `sai`

- "check mainnet BTC/USDC OI right now and tell me whether pair OI or group OI is the binding limit. use contract state, not the API"
- "why does `GET /dexpal/v1/stats` say open interest is 0 even though I can see open trades in the app? compare REST, DB, and contract state"
- "for trader `nibi1...`, find open Sai positions, fee multiplier, and whether they have any trading credits"
- "can you compare the REST 24h volume against table `stats_perp_by_user` and explain any timing mismatch?" (the table half needs private agent skill `sai-db`)
- "does referral code `FOO` exist, who owns it, and does GraphQL show any redemptions or referred trades? don't create anything"
- "inspect SLP USDC vault health: collateralization, share price, max deposit, current epoch, and raw PnL state if it looks weird"
- "what GraphQL field should I use to subscribe to live perp trade updates for a trader, and how do the amounts scale?"
- "map Sai market index 1002 to its token, group, fee index, and current oracle price"
- "the app shows a different vault TVL than the contract query. help me reconcile GraphQL, REST, DB, and on-chain values"

## Should Not Trigger `sai`

- "upload the latest `perp.wasm`, compare the checksum to on-chain code, and migrate mainnet if needed" -> use `sai-ops`.
- "create referral code `FOO` for this EVM address and update the sheet afterward" -> use `sai-ops`.
- "issue 300 USDC of Sai trading credits to this affiliate" -> use `sai-ops`.
- "run the trigger-trades admin flow for this keeper batch" -> use `sai-ops`.
- "which `sai-keeper` table backs referral redemption history?" -> use `sai-db`.
- "debug a raw Nibiru EVM tx receipt unrelated to Sai contracts" -> use `evm-rpc` or `nibiru-cli`.
- "query a normal Nibiru wallet balance for staking rewards" -> use Nibiru/indexer balance or staking skills.
