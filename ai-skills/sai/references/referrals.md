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

## Read-Only Testnet Balance Recipe

Use this recipe to check which Testnet2 accounts hold Sai testnet USDC in the
wallet, withdrawable Sai deposits, or Sai trading credits. It only reads.

It checks:

- Wallet USDC with `nibiru q evm balance`.
- Sai withdrawable deposits and non-withdrawable trading credits with Perp
  query `list_user_deposits`.

### Network and Constants

Pass the network on every query. Do not run `nibiru config node` or `nibiru
config chain-id` for this: those commands rewrite `client.toml`, and the
person's next unqualified command would go to Testnet2.

```bash
NET=(--node https://rpc.testnet-2.nibiru.fi:443 --chain-id nibiru-testnet-2)
PERP="nibi1qtkcns647w959cj9x2yytateu6dgscfnfkraywwa443pr2erak0s5ux7e5"
```

Do not hardcode the USDC denom. Testnet2 has had more than one USDC asset,
and a wallet funded with the one Sai uses shows zero under another. Ask the
contract which asset it uses: `get_trading_credit_params` names the USDC
collateral index, and `get_collateral` returns that collateral's wallet
denom. On 2026-11-19 these returned `TokenIndex(3)` and
`erc20/0x1d2eD546C1b5e142d568cab5a4A438f3B2f561E3`.

This recipe reads current state, so the default non-archive RPC is enough.
Use the base-unit fields from `nibiru q evm balance`.

### Summary Command

Run this with `nibiru`, `jq`, and `python3` on `PATH`. List the Bech32
addresses to check in `ADDRS`. `list_user_deposits` returns one row per
collateral, so the script reads the USDC collateral index from the contract,
reads only that row, and queries the wallet for that collateral's denom.

```bash
set -euo pipefail

NET=(--node https://rpc.testnet-2.nibiru.fi:443 --chain-id nibiru-testnet-2)
PERP="nibi1qtkcns647w959cj9x2yytateu6dgscfnfkraywwa443pr2erak0s5ux7e5"
ADDRS=(nibi1... nibi1...)

perp_q() {
  nibiru q wasm contract-state smart "$PERP" "$1" "${NET[@]}" --output json
}

# The collateral that trading credits and USDC deposits use, such as
# "TokenIndex(3)", and the wallet denom the contract accepts for it.
usdc_idx=$(perp_q '{"get_trading_credit_params":{}}' |
  jq -r '.data.collateral_idx')
usdc_num=$(echo "$usdc_idx" | tr -cd '0-9')
USDC=$(perp_q '{"get_collateral":{"index":'"$usdc_num"'}}' |
  jq -r '.data.denom')
echo "Sai USDC collateral: $usdc_idx, wallet denom: $USDC"

printf "%-46s %14s %14s %16s %16s\n" \
  "ADDR" "BANK_USDC" "ERC20_USDC" "SAI_DEPOSIT" "SAI_CREDITS"

for bech32 in "${ADDRS[@]}"; do
  bal=$(nibiru q evm balance "$bech32" "$USDC" "${NET[@]}" --output json)
  bank_base=$(echo "$bal" | jq -r '.bank_balance_base // "0"')
  erc20_base=$(echo "$bal" | jq -r '.erc20_balance_base // "0"')

  deposits=$(perp_q '{"list_user_deposits":{"user":"'"$bech32"'"}}')
  row=$(echo "$deposits" | jq --arg idx "$usdc_idx" \
    '[.data[]? | select(.collateral_index == $idx)] | first // {}')
  sai_deposit=$(echo "$row" | jq -r '.amount // "0"')
  sai_credits=$(echo "$row" | jq -r '.trading_credits // "0"')

  bank_usdc=$(python3 -c \
    "print(f'{int(\"$bank_base\") / 1_000_000:,.2f}')")

  printf "%-46s %14s %14s %16s %16s\n" \
    "$bech32" "$bank_usdc" "$erc20_base" "$sai_deposit" "$sai_credits"
done
```

To build `ADDRS` from the local keyring, ask for JSON explicitly, because
`nibiru keys list` prints text by default:

```bash
nibiru keys list --output json | jq -r '.[] | [.name, .address] | @tsv'
```

Check only the accounts the person names. Do not enumerate the whole keyring
in a report.

### Reading the Output

- `BANK_USDC`: wallet USDC held as the Cosmos bank token, in 6-decimal USDC
  units.
- `ERC20_USDC`: wallet USDC held in the ERC20 representation, in base units.
- `SAI_DEPOSIT`: withdrawable Sai USDC collateral balance, in base units.
- `SAI_CREDITS`: non-withdrawable Sai trading credit balance, in USDC base
  units.

Mention only rows with a balance. If every Sai deposit and credit field is
zero, say that none of the checked accounts holds a Sai USDC deposit or
trading credits.

### Optional Verification

For any nonzero row, read the wallet and the contract directly, with `USDC`
set as the script sets it:

```bash
BECH32="nibi1..."

nibiru q evm balance "$BECH32" "$USDC" "${NET[@]}" --output json | jq '{
  bank_balance_base,
  erc20_balance_base
}'

perp_q '{"list_user_deposits":{"user":"'"$BECH32"'"}}' | jq '.data'
```

To map between the Bech32 and EVM forms of an account:

```bash
nibiru q evm account "$BECH32" "${NET[@]}" --output json | jq '{
  bech32_address,
  eth_address,
  balance_wei
}'
```
