# nibid-gov-upgrade: reference

Extended reference for [`SKILL.md`](./SKILL.md). This file is intentionally
longer and optimized for “copy/paste” and quick lookup.

## Assumptions

- **Network**: mainnet via `ud nibi cfg prod`
- **Output**: `ud nibi cfg prod` sets `nibid config output json`, so examples
  omit `-o json`
- **Tools**: `jq` installed

Quick setup:

```bash
ud nibi cfg prod
nibid config
ID=27
```

## Proposal JSON shapes (why the fields look the way they do)

On Nibiru mainnet, software upgrades show up as a **gov v1** proposal containing
`messages[]`, typically with a legacy wrapper:

- `messages[].@type == "/cosmos.gov.v1.MsgExecLegacyContent"`
- `messages[].content.@type == "/cosmos.upgrade.v1beta1.SoftwareUpgradeProposal"`

So the upgrade plan lives under:

- `.messages[] | ... | .content.plan`

Other chains / older SDK versions may still surface “legacy content proposals”
under a single `content` field (gov v1beta1). When you’re scripting, handle both
shapes.

## Explorer UI → on-chain mapping (commands + JSON paths)

| Explorer UI area | Command | Primary JSON paths / notes |
|---|---|---|
| Overview (title/summary/status) | `nibid q gov proposal "$ID"` | `.title`, `.summary`, `.status` |
| Voting start/end | `nibid q gov proposal "$ID"` | `.voting_start_time`, `.voting_end_time` |
| Submit/deposit times | `nibid q gov proposal "$ID"` | `.submit_time`, `.deposit_end_time` |
| Proposer | `nibid q gov proposal "$ID"` or `nibid q gov proposer "$ID"` | `.proposer` |
| Total deposit | `nibid q gov proposal "$ID"` | `.total_deposit[]` |
| Deposits tab | `nibid q gov deposits "$ID"` | `.deposits[]` |
| Votes tab | `nibid q gov votes "$ID" --limit 1000` | `.votes[]` (weighted `options[]`) |
| Tally totals | `nibid q gov tally "$ID"` | `.yes_count`, `.no_count`, `.abstain_count`, `.no_with_veto_count` |
| Quorum/threshold params | `nibid q gov params` or `nibid q gov param tallying` | `.tally_params.quorum`, `.threshold`, `.veto_threshold` |
| Voting/deposit params | `nibid q gov params` or `nibid q gov param voting|deposit` | `voting_period`, `min_deposit`, `max_deposit_period` |
| Bonded denominator for quorum | `nibid q staking pool` | `.bonded_tokens` |
| Validator set (w/ power) | `nibid q staking validators --limit 2000` | `.validators[]` includes `.tokens`, `.status`, `.operator_address` |
| Upgrade plan (name/height/info) | `nibid q gov proposal "$ID"` | see extraction recipes below |
| Scheduled upgrade plan | `nibid q upgrade plan` | Only after chain schedules an upgrade; can return “no upgrade scheduled” during voting |

## Canonical query bundle (proposal-centric)

```bash
nibid q gov proposal "$ID"
nibid q gov votes "$ID" --limit 1000
nibid q gov tally "$ID"
nibid q gov deposits "$ID"
nibid q gov params
nibid q staking pool
nibid q staking validators --limit 2000
```

## Recipes

### Extract the software-upgrade plan (proposal content)

```bash
nibid q gov proposal "$ID" | jq '
  .messages[]
  | select(."@type" == "/cosmos.gov.v1.MsgExecLegacyContent")
  | .content
  | select(."@type" == "/cosmos.upgrade.v1beta1.SoftwareUpgradeProposal")
  | .plan
'
```

### Parse `plan.info` and extract `binaries`

`plan.info` is a JSON *string* (Cosmovisor-compatible metadata is a common
convention).

```bash
nibid q gov proposal "$ID" | jq -r '
  .messages[]
  | select(."@type" == "/cosmos.gov.v1.MsgExecLegacyContent")
  | .content
  | select(."@type" == "/cosmos.upgrade.v1beta1.SoftwareUpgradeProposal")
  | .plan.info
  | fromjson
  | .binaries
'
```

### Compute quorum / threshold / veto like explorers

Conceptually:

- **Quorum**: \((yes+no+abstain+veto) / bonded_tokens\)
- **Threshold**: \(yes / (yes+no+veto)\) (abstain excluded)
- **Veto%**: \(veto / (yes+no+veto)\) (abstain excluded)

Copy/paste:

```bash
jq -s '
  .[0] as $t
  | .[1] as $p
  | ($p.bonded_tokens|tonumber) as $bonded
  | ($t.yes_count|tonumber) as $yes
  | ($t.no_count|tonumber) as $no
  | ($t.abstain_count|tonumber) as $abstain
  | ($t.no_with_veto_count|tonumber) as $veto
  | ($yes+$no+$abstain+$veto) as $total
  | {
      bonded_tokens: $bonded,
      total_voted_tokens: $total,
      quorum_pct: (if $bonded==0 then null else ($total / $bonded * 100) end),
      yes_pct_of_non_abstain: (if ($yes+$no+$veto)==0 then null else ($yes / ($yes+$no+$veto) * 100) end),
      veto_pct_of_non_abstain: (if ($yes+$no+$veto)==0 then null else ($veto / ($yes+$no+$veto) * 100) end)
    }
' <(nibid q gov tally "$ID") <(nibid q staking pool)
```

### Pagination patterns (votes, deposits, validators)

Most list queries support (varying by command):

- `--limit`, `--page`, `--offset`, `--page-key`, `--count-total`, `--reverse`

Examples:

```bash
# Votes
nibid q gov votes "$ID" --limit 100 --page 1
nibid q gov votes "$ID" --limit 100 --page 2

# Deposits
nibid q gov deposits "$ID" --limit 100 --page 1

# Validators
nibid q staking validators --limit 100 --page 1
```

### Validator vote report (join gov votes to validators)

Problem: `nibid q gov votes "$ID"` includes **all** voters, not just validators.
To filter to validators, you need to map each validator operator address
(`nibivaloper...`) to its corresponding account address (`nibi...`).

Nibiru has built-in helpers:

```bash
nibid debug prefixes
nibid debug addr nibivaloper1...
```

`nibid debug addr` prints lines like:

- `Bech32 Acc: nibi1...`
- `Bech32 Val: nibivaloper1...`

Example workflow (bonded validators who voted):

```bash
VOTES_JSON="$(nibid q gov votes "$ID" --limit 1000)"
VALS_JSON="$(nibid q staking validators --limit 2000)"

echo "$VALS_JSON" \
  | jq -r '.validators[] | select(.status=="BOND_STATUS_BONDED") | .operator_address' \
  | while read -r valoper; do
      acc="$(nibid debug addr "$valoper" | sed -n 's/^Bech32 Acc: //p')"
      vote="$(echo "$VOTES_JSON" | jq -c --arg acc "$acc" '.votes[] | select(.voter==$acc) | {voter, options}')"
      if [[ -n "$vote" ]]; then
        moniker="$(echo "$VALS_JSON" | jq -r --arg valoper "$valoper" '.validators[] | select(.operator_address==$valoper) | .description.moniker')"
        tokens="$(echo "$VALS_JSON" | jq -r --arg valoper "$valoper" '.validators[] | select(.operator_address==$valoper) | .tokens')"
        echo "$moniker $valoper $acc tokens=$tokens vote=$vote"
      fi
    done
```

Notes / pitfalls:

- `staking validators` includes unbonded/unbonding validators; explorers often
  display only the bonded set.
- Voting power in governance is delegation-weighted; validator “tokens” here are
  a proxy for validator voting power, not a direct “vote weight” per voter.

## Transactions reference (gov)

### Vote

```bash
nibid tx gov vote "$ID" yes --from <key-or-addr>
```

### Weighted vote

```bash
nibid tx gov weighted-vote "$ID" yes=0.6,no=0.3,abstain=0.1 --from <key-or-addr>
```

### Deposit

```bash
nibid tx gov deposit "$ID" 1000000unibi --from <key-or-addr>
```

### Submit proposal (gov v1)

`nibid tx gov submit-proposal` takes a proposal JSON file containing `messages`
(proto-JSON `sdk.Msg`s) plus metadata and deposit information.

See:

```bash
nibid tx gov submit-proposal --help
```

### Submit software-upgrade proposal (legacy content path)

```bash
nibid tx gov submit-legacy-proposal software-upgrade v2.11.0 \
  --title "v2.11.0" \
  --description "Upgrade to v2.11.0" \
  --deposit 20000000000unibi \
  --upgrade-height 36757700 \
  --upgrade-info '{"binaries":{"linux/amd64":"https://...","linux/arm64":"https://...","docker":"ghcr.io/...:2.11.0"}}' \
  --from <key-or-addr>
```

## Upgrade module vs proposal content (common confusion)

- `nibid q gov proposal "$ID"` always shows what’s being voted on, including an
  upgrade plan embedded in proposal messages.
- `nibid q upgrade plan` queries the chain’s scheduled upgrade plan in `x/upgrade`.
  If the proposal hasn’t passed / been scheduled yet, it can return:

  `Error: no upgrade scheduled`

So: treat the proposal as the source of truth while voting is ongoing.

## Links

- Nibiru governance lifecycle: `https://nibiru.fi/docs/community/governance.html`
- Nibiru submitting proposals: `https://nibiru.fi/docs/community/submitting-proposals.html`
- Cosmos SDK upgrade module: `https://docs.cosmos.network/v0.53/build/modules/upgrade`

