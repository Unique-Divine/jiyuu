# nibiru-gov-upgrade: reference

Extended reference for [`SKILL.md`](./SKILL.md). This file is intentionally
longer and optimized for “copy/paste” and quick lookup.

## Assumptions

- **Network**: mainnet via `ud nibi cfg prod`, which selects the default,
  non-archive RPC
- **Archive**: use `ud nibi cfg prod --archive` only for historical read-only
  data that the standard RPC has pruned, then restore `ud nibi cfg prod`
- **Output**: `ud nibi cfg prod` runs `nibiru config output json`, so examples
  omit `-o json`
- **Tools**: `jq` installed

Quick setup:

```bash
ud nibi cfg prod
nibiru config
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
| Overview (title/summary/status) | `nibiru q gov proposal "$ID"` | `.title`, `.summary`, `.status` |
| Voting start/end | `nibiru q gov proposal "$ID"` | `.voting_start_time`, `.voting_end_time` |
| Submit/deposit times | `nibiru q gov proposal "$ID"` | `.submit_time`, `.deposit_end_time` |
| Proposer | `nibiru q gov proposal "$ID"` or `nibiru q gov proposer "$ID"` | `.proposer` |
| Total deposit | `nibiru q gov proposal "$ID"` | `.total_deposit[]` |
| Deposits tab | `nibiru q gov deposits "$ID"` | `.deposits[]` |
| Votes tab | `nibiru q gov votes "$ID" --limit 1000` | `.votes[]` (weighted `options[]`) |
| Tally totals | `nibiru q gov tally "$ID"` | `.yes_count`, `.no_count`, `.abstain_count`, `.no_with_veto_count` |
| Quorum/threshold params | `nibiru q gov params` or `nibiru q gov param tallying` | `.tally_params.quorum`, `.threshold`, `.veto_threshold` |
| Voting/deposit params | `nibiru q gov params` or `nibiru q gov param voting|deposit` | `voting_period`, `min_deposit`, `max_deposit_period` |
| Bonded denominator for quorum | `nibiru q staking pool` | `.bonded_tokens` |
| Validator set (w/ power) | `nibiru q staking validators --limit 2000` | `.validators[]` includes `.tokens`, `.status`, `.operator_address` |
| Upgrade plan (name/height/info) | `nibiru q gov proposal "$ID"` | see extraction recipes below |
| Scheduled upgrade plan | `nibiru q upgrade plan` | Only after chain schedules an upgrade; can return “no upgrade scheduled” during voting |

## Canonical query bundle (proposal-centric)

```bash
nibiru q gov proposal "$ID"
nibiru q gov votes "$ID" --limit 1000
nibiru q gov tally "$ID"
nibiru q gov deposits "$ID"
nibiru q gov params
nibiru q staking pool
nibiru q staking validators --limit 2000
```

## Recipes

### Extract the software-upgrade plan (proposal content)

```bash
nibiru q gov proposal "$ID" | jq '
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
nibiru q gov proposal "$ID" | jq -r '
  .messages[]
  | select(."@type" == "/cosmos.gov.v1.MsgExecLegacyContent")
  | .content
  | select(."@type" == "/cosmos.upgrade.v1beta1.SoftwareUpgradeProposal")
  | .plan.info
  | fromjson
  | .binaries
'
```

### Quorum, threshold, and veto

Conceptually:

- **Quorum**: \((yes+no+abstain+veto) / bonded_tokens\)
- **Threshold**: \(yes / (yes+no+veto)\) (abstain excluded)
- **Veto%**: \(veto / (yes+no+veto)\) (abstain excluded)

Inputs:

- `nibiru q gov tally "$ID"` gives the current vote weights:
  `yes_count`, `no_count`, `abstain_count`, `no_with_veto_count`
- `nibiru q gov params` gives the governance thresholds:
  `quorum`, `threshold`, `veto_threshold`
- `nibiru q staking pool` gives `bonded_tokens`, the quorum denominator

### Explorer-style quorum report

```bash
TALLY="$(nibiru q gov tally "$ID")"
PARAMS="$(nibiru q gov params)"
POOL="$(nibiru q staking pool)"

jq -n \
  --argjson tally "$TALLY" \
  --argjson params "$PARAMS" \
  --argjson pool "$POOL" '
  ($tally.yes_count|tonumber) as $yes
  | ($tally.no_count|tonumber) as $no
  | ($tally.abstain_count|tonumber) as $abstain
  | ($tally.no_with_veto_count|tonumber) as $veto
  | ($pool.bonded_tokens|tonumber) as $bonded
  | ($yes+$no+$abstain+$veto) as $total
  | {
      tally: $tally,
      bonded_tokens: $pool.bonded_tokens,
      quorum_required_pct: (($params.tally_params.quorum|tonumber) * 100),
      threshold_required_pct: (($params.tally_params.threshold|tonumber) * 100),
      veto_threshold_pct: (($params.tally_params.veto_threshold|tonumber) * 100),
      total_voted_tokens: $total,
      quorum_pct: (if $bonded==0 then null else ($total / $bonded * 100) end),
      yes_pct_of_non_abstain: (
        if ($yes+$no+$veto)==0 then null
        else ($yes / ($yes+$no+$veto) * 100)
        end
      ),
      veto_pct_of_non_abstain: (
        if ($yes+$no+$veto)==0 then null
        else ($veto / ($yes+$no+$veto) * 100)
        end
      )
    }'
```

### Field meanings

- `tally`: raw output copied from `nibiru q gov tally "$ID"`
- `bonded_tokens`: raw output copied from `nibiru q staking pool`
- `quorum_required_pct`: derived from `nibiru q gov params` by converting
  `.tally_params.quorum` from a decimal fraction to percent
- `threshold_required_pct`: derived from `nibiru q gov params` by converting
  `.tally_params.threshold` from a decimal fraction to percent
- `veto_threshold_pct`: derived from `nibiru q gov params` by converting
  `.tally_params.veto_threshold` from a decimal fraction to percent
- `total_voted_tokens`: `yes + no + abstain + veto`
- `quorum_pct`: `total_voted_tokens / bonded_tokens * 100`
- `yes_pct_of_non_abstain`: `yes / (yes + no + veto) * 100`
- `veto_pct_of_non_abstain`: `veto / (yes + no + veto) * 100`

Notes:

- `abstain` counts toward quorum participation, so it is included in
  `total_voted_tokens`.
- `abstain` is excluded from `yes_pct_of_non_abstain` and
  `veto_pct_of_non_abstain` to match governance threshold semantics and typical
  explorer displays.

### Pagination patterns (votes, deposits, validators)

Most list queries support (varying by command):

- `--limit`, `--page`, `--offset`, `--page-key`, `--count-total`, `--reverse`

Examples:

```bash
# Votes
nibiru q gov votes "$ID" --limit 100 --page 1
nibiru q gov votes "$ID" --limit 100 --page 2

# Deposits
nibiru q gov deposits "$ID" --limit 100 --page 1

# Validators
nibiru q staking validators --limit 100 --page 1
```

### Validator vote report (join gov votes to validators)

Problem: `nibiru q gov votes "$ID"` includes **all** voters, not just validators.
To filter to validators, you need to map each validator operator address
(`nibivaloper...`) to its corresponding account address (`nibi...`).

Nibiru has built-in helpers:

```bash
nibiru debug prefixes
nibiru debug addr nibivaloper1...
```

`nibiru debug addr` prints lines like:

- `Bech32 Acc: nibi1...`
- `Bech32 Val: nibivaloper1...`

Example workflow (bonded validators who voted):

```bash
VOTES_JSON="$(nibiru q gov votes "$ID" --limit 1000)"
VALS_JSON="$(nibiru q staking validators --limit 2000)"

echo "$VALS_JSON" \
  | jq -r '.validators[] | select(.status=="BOND_STATUS_BONDED") | .operator_address' \
  | while read -r valoper; do
      acc="$(nibiru debug addr "$valoper" | sed -n 's/^Bech32 Acc: //p')"
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
nibiru tx gov vote "$ID" yes --from <key-or-addr>
```

### Weighted vote

```bash
nibiru tx gov weighted-vote "$ID" yes=0.6,no=0.3,abstain=0.1 --from <key-or-addr>
```

### Deposit

```bash
nibiru tx gov deposit "$ID" 1000000unibi --from <key-or-addr>
```

### Submit proposal (gov v1)

`nibiru tx gov submit-proposal` takes a proposal JSON file containing `messages`
(proto-JSON `sdk.Msg`s) plus metadata and deposit information.

See:

```bash
nibiru tx gov submit-proposal --help
```

### Submit software-upgrade proposal (legacy content path)

```bash
nibiru tx gov submit-legacy-proposal software-upgrade v2.11.0 \
  --title "v2.11.0" \
  --description "Upgrade to v2.11.0" \
  --deposit 20000000000unibi \
  --upgrade-height 36757700 \
  --upgrade-info '{"binaries":{"linux/amd64":"https://...","linux/arm64":"https://...","docker":"ghcr.io/...:2.11.0"}}' \
  --from <key-or-addr>
```

## Upgrade module vs proposal content (common confusion)

- `nibiru q gov proposal "$ID"` always shows what’s being voted on, including an
  upgrade plan embedded in proposal messages.
- `nibiru q upgrade plan` queries the chain’s scheduled upgrade plan in `x/upgrade`.
  If the proposal hasn’t passed / been scheduled yet, it can return:

  `Error: no upgrade scheduled`

So: treat the proposal as the source of truth while voting is ongoing.

## Links

- Nibiru governance lifecycle: `https://nibiru.fi/docs/community/governance.html`
- Nibiru submitting proposals: `https://nibiru.fi/docs/community/submitting-proposals.html`
- Cosmos SDK upgrade module: `https://docs.cosmos.network/v0.53/build/modules/upgrade`
