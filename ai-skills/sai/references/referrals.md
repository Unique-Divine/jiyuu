# Sai referral queries

Use this reference for referral and affiliate diagnostics: existing code
ownership, code redemption history, EVM/Bech32 address
normalization, indexed referral analytics, and trading-credit balance checks.

If the user asks to create or update referral codes, set discounts, issue
trading credits, or edit operator records, switch to agent skill `sai-ops`.

## Source-of-truth model

1. On-chain Sai perp contract storage owns referral-code ownership,
   code-specific discounts, current referral relationships, and Sai internal
   user deposit or trading-credit balances. The current Perp smart-query API
   does not expose a general referral-code ownership lookup; do not claim a
   smart-query result for an arbitrary code without a verified raw-storage or
   operator procedure.
2. Sai Keeper GraphQL and the `sai-keeper` Postgres database are truth for
   indexed historical analytics such as redemptions, trades, volume, earnings,
   and claims.
3. Operator records, such as a referral spreadsheet, are human-maintained
   metadata: names, deal terms, share URLs, and labels. Agent skill `sai-ops`
   covers them. Do not use them as proof that a code exists or that an address
   owns it.

## Constants

- A referral link is `https://sai.fun/?ref=<code>`.
- Mainnet Perp contract: `nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph`

## Existing affiliate information

When asked about an existing code, address, or partner:

1. Use GraphQL referral code and redemption records for indexed ownership and
   history. Escalate to a verified raw-state/operator procedure only when the
   question specifically requires contract storage as the deciding layer.
2. Normalize EVM and Bech32 address pairs with command `nibiru q evm account`.
3. Check code-specific discount state if fee terms matter.
4. Use Sai Keeper GraphQL for indexed history or analytics: redemptions, referred
   trades, volume, earnings, stats, or claims.
5. When GraphQL/REST behavior or table ownership is the question, use
   private agent skill `sai-db`. Relevant tables include table `referral_code`, table
   `referral_redeem_history`, table `referral_claim_history`, and the referral
   stats tables.

## Related query surfaces

- Contract query payloads and constants: [`contracts.md`](contracts.md)
- GraphQL fields `referral.referralCodes`, `referral.referralRedemption`, and
  `referral.referralRedemptionTrader`:
  [`graphql.md`](graphql.md)
- REST endpoint `GET /dexpal/v1/referrals`: [`rest.md`](rest.md)
- DB referral tables and API surfacing: private agent skill `sai-db`

---

## Read-only testnet balance recipe

### Testnet USDC and Sai deposit balance checks

Use this recipe when an operator asks which local testnet accounts have
canonical Sai testnet USDC in the wallet, withdrawable Sai perp deposits, or
Sai trading credits. Keep the report scoped to local keyring names with the
`test-` prefix; do not enumerate unrelated local keyring accounts.

This is a read-only workflow. It checks:

- Wallet USDC via `nibiru q evm balance`.
- Sai internal withdrawable deposits via `list_user_deposits`.
- Sai internal non-withdrawable credits via `list_user_deposits`.

#### Network and constants

Point `nibiru` at Testnet2 and verify before querying:

```bash
nibiru config node https://rpc.testnet-2.nibiru.fi:443
nibiru config chain-id nibiru-testnet-2
nibiru config
```

This recipe reads current EVM and Wasm state. Keep the default, non-archive
testnet RPC; do not add `--archive`.

Expected chain:

```text
nibiru-testnet-2
```

Use these testnet constants:

```bash
PERP="nibi1qtkcns647w959cj9x2yytateu6dgscfnfkraywwa443pr2erak0s5ux7e5"
USDC="tf/nibi1pc2mmwcqhvzn9vsm0umpu40yzl6gfy6nucwn7g/usdc"
```

`USDC` is the canonical Sai testnet USDC token-factory denom. Use the base-unit
fields from `nibiru q evm balance`; on testnet, token metadata can make human
fields misleading.

#### Summary command

Run this from any directory with `nibiru`, `jq`, and `python3` on `PATH`:

```bash
set -euo pipefail

PERP="nibi1qtkcns647w959cj9x2yytateu6dgscfnfkraywwa443pr2erak0s5ux7e5"
USDC="tf/nibi1pc2mmwcqhvzn9vsm0umpu40yzl6gfy6nucwn7g/usdc"

printf "%-24s %14s %14s %16s %16s\n" \
  "KEY" "BANK_USDC" "ERC20_USDC" "SAI_DEPOSIT" "SAI_CREDITS"

nibiru keys list |
  jq -r '.[] | select(.type == "local" and (.name | startswith("test-"))) |
    [.name, .address] | @tsv' |
  while IFS="$(printf '\t')" read -r name bech32; do
    bal=$(nibiru q evm balance "$bech32" "$USDC")
    bank_base=$(echo "$bal" | jq -r '.bank_balance_base // "0"')
    erc20_base=$(echo "$bal" | jq -r '.erc20_balance_base // "0"')

    query='{"list_user_deposits":{"user":"'"$bech32"'"}}'
    deposits=$(nibiru q wasm contract-state smart "$PERP" "$query")
    sai_deposit=$(echo "$deposits" |
      jq -r '[.data[]? | (.amount // "0" | tonumber)] | add // 0')
    sai_credits=$(echo "$deposits" |
      jq -r '[.data[]? | (.trading_credits // "0" | tonumber)] | add // 0')

    bank_usdc=$(python3 -c \
      "print(f'{int(\"$bank_base\") / 1_000_000:,.2f}')")

    printf "%-24s %14s %14s %16s %16s\n" \
      "$name" "$bank_usdc" "$erc20_base" "$sai_deposit" "$sai_credits"
  done
```

#### Reading the output

Interpret the columns this way:

- `BANK_USDC`: wallet USDC held as the Cosmos bank token, displayed with
  6-decimal USDC units.
- `ERC20_USDC`: wallet USDC held in the ERC20 representation, still in base
  units.
- `SAI_DEPOSIT`: withdrawable Sai perp collateral balance, in USDC base units.
- `SAI_CREDITS`: non-withdrawable Sai trading credit balance, in USDC base
  units.

Mention only rows with meaningful balances when answering an operator question.
If all Sai deposit and credit fields are zero, say that no `test-*` keyring
account currently has Sai internal USDC deposit balance or trading credits.

#### Optional verification

For any nonzero row, verify the raw wallet and Sai contract state directly:

```bash
BECH32="nibi1..."

nibiru q evm balance "$BECH32" "$USDC" | jq '{
  bank_balance_base,
  erc20_balance_base
}'

nibiru q wasm contract-state smart "$PERP" \
  '{"list_user_deposits":{"user":"'"$BECH32"'"}}' |
  jq '.data'
```

If the operator asks for account normalization, use:

```bash
nibiru q evm account "$BECH32" | jq '{
  bech32_address,
  eth_address,
  balance_wei
}'
```
