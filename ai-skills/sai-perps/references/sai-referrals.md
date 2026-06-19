# Sai referral queries

Use this reference for referral and affiliate diagnostics: existing code
ownership, code redemption history, sheet metadata, EVM/Bech32 address
normalization, indexed referral analytics, and trading-credit balance checks.

If the user asks to create or update referral codes, set discounts, issue
trading credits, mutate the Referral Identity Directory sheet, or append operator
changelog notes, switch to agent skill `sai-ops`.

## Source-of-truth model

1. On-chain Sai perp contract state is truth for current referral code ownership,
   code-specific discounts, current referral relationships, and Sai internal
   user deposit or trading-credit balances.
2. Sai Keeper GraphQL and the `sai-keeper` Postgres database are truth for
   indexed historical analytics such as redemptions, trades, volume, earnings,
   and claims.
3. The Referral Identity Directory sheet is human-maintained metadata: Telegram
   names, Notion CRM links, notes, deal terms, share URLs, and operator labels.

## Important paths

- Sai referrals epic: `/home/realu/ki/boku/epics/tools/sai-deal-desk`
- Sheet helper script: `/home/realu/ki/boku/epics/tools/sai-deal-desk/cli.ts`
- Sai perp repo: `/home/realu/ki/sai-perps`
- Mainnet perp contract: `nibi1ntmw2dfvd0qnw5fnwdu9pev2hsnqfdj9ny9n0nzh2a5u8v0scflq930mph`

## Referral Directory sheet

Current columns:

```text
saiCode, telegramGroupName, notionCrmUrl, notes, hasDeal, shareUrl,
addrEvm, addrBech32, txHashCodeCreation, accType
```

Interpretation rules:

- Preserve `saiCode` as the exact referral code identifier.
- Derive `shareUrl` as `https://sai.fun/?ref=<saiCode>` when checking
  whether a sheet value is stale.
- Treat the sheet as metadata. Do not use it as proof that a code exists or that
  an address owns the code.
- Explain conflicts by source: sheet metadata, on-chain owner, and indexed
  GraphQL/Postgres history.

## Existing affiliate information

When asked about an existing code, address, or partner:

1. Search the sheet or epic for exact code/address matches and likely human
   metadata matches.
2. Cross-check on-chain ownership through the Sai perp contract.
3. Normalize EVM and Bech32 address pairs with command `nibid q evm account`.
4. Check code-specific discount state if fee terms matter.
5. Use Sai Keeper GraphQL for indexed history or analytics: redemptions, referred
   trades, volume, earnings, stats, or claims.
6. Use DB references when GraphQL/REST behavior or table ownership is the
   question. Relevant tables include table `referral_code`, table
   `referral_redeem_history`, table `referral_claim_history`, and referral stats
   tables documented in [`sai-db.md`](sai-db.md).

## Related query surfaces

- Contract query payloads and constants: [`sai-contracts.md`](sai-contracts.md)
- GraphQL fields `referralInfo`, `referralCodes`, and `referralRedemption`:
  [`sai-graphql.md`](sai-graphql.md)
- REST endpoint `GET /dexpal/v1/referrals`: [`sai-rest.md`](sai-rest.md)
- DB referral tables and API surfacing: [`sai-db.md`](sai-db.md)

---

## Read-only testnet balance recipe

### Testnet USDC and Sai deposit balance checks

Use this recipe when an operator asks which local testnet accounts have
canonical Sai testnet USDC in the wallet, withdrawable Sai perp deposits, or
Sai trading credits. Keep the report scoped to local keyring names with the
`test-` prefix; do not enumerate unrelated local keyring accounts.

This is a read-only workflow. It checks:

- Wallet USDC via `nibid q evm balance`.
- Sai internal withdrawable deposits via `list_user_deposits`.
- Sai internal non-withdrawable credits via `list_user_deposits`.

#### Network and constants

Point `nibid` at Testnet2 and verify before querying:

```bash
ud nibi cfg test
nibid config
```

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
fields from `nibid q evm balance`; on testnet, token metadata can make human
fields misleading.

#### Summary command

Run this from any directory with `nibid`, `jq`, and `python3` on `PATH`:

```bash
set -euo pipefail

PERP="nibi1qtkcns647w959cj9x2yytateu6dgscfnfkraywwa443pr2erak0s5ux7e5"
USDC="tf/nibi1pc2mmwcqhvzn9vsm0umpu40yzl6gfy6nucwn7g/usdc"

printf "%-24s %14s %14s %16s %16s\n" \
  "KEY" "BANK_USDC" "ERC20_USDC" "SAI_DEPOSIT" "SAI_CREDITS"

nibid keys list |
  jq -r '.[] | select(.type == "local" and (.name | startswith("test-"))) |
    [.name, .address] | @tsv' |
  while IFS="$(printf '\t')" read -r name bech32; do
    bal=$(nibid q evm balance "$bech32" "$USDC")
    bank_base=$(echo "$bal" | jq -r '.bank_balance_base // "0"')
    erc20_base=$(echo "$bal" | jq -r '.erc20_balance_base // "0"')

    query='{"list_user_deposits":{"user":"'"$bech32"'"}}'
    deposits=$(nibid q wasm contract-state smart "$PERP" "$query")
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

nibid q evm balance "$BECH32" "$USDC" | jq '{
  bank_balance_base,
  erc20_balance_base
}'

nibid q wasm contract-state smart "$PERP" \
  '{"list_user_deposits":{"user":"'"$BECH32"'"}}' |
  jq '.data'
```

If the operator asks for account normalization, use:

```bash
nibid q evm account "$BECH32" | jq '{
  bech32_address,
  eth_address,
  balance_wei
}'
```
