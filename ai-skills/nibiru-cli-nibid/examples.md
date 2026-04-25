# `nibid` Examples

Concrete copy-paste examples for [`SKILL.md`](./SKILL.md).

## Bank send with memo (`--note`)

Use this when the destination requires a memo or reference code, such as an
exchange deposit account.

```bash
FROM="ud-prod"
TO_ADDR="nibi1recipient..."
MEMO="2097215673"
AMOUNT="1000000unibi"

nibid tx bank send \
  "$FROM" \
  "$TO_ADDR" \
  "$AMOUNT" \
  --note "$MEMO" \
  --fees "5000unibi" \
  -y
```

Safer pre-flight variant:

```bash
FROM="ud-prod"
TO_ADDR="nibi1recipient..."
MEMO="2097215673"
AMOUNT="1000000unibi"

nibid tx bank send \
  "$FROM" \
  "$TO_ADDR" \
  "$AMOUNT" \
  --note "$MEMO" \
  --chain-id "cataclysm-1" \
  --node "https://rpc.archive.nibiru.fi:443" \
  --fees "5000unibi" \
  --gas auto \
  --gas-adjustment 1.3 \
  --dry-run
```

Notes:

- Use `--note`, not a guessed memo flag.
- Keep the amount in base units such as `unibi` unless the user explicitly wants
  a human-unit conversion.
- Verify the active network and `--from` value before broadcasting.
