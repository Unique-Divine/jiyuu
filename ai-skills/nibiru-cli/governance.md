# Governance and software-upgrade proposals

Query Nibiru governance proposals, tallies, and upgrade plans with the `nibiru`
CLI. This reference covers reading proposal state, computing quorum and
thresholds, filtering validator votes, and casting votes or deposits.

Creating and operating a software-upgrade proposal is an operator workflow
outside this public skill.

## Contents

- [Select a network](#select-a-network)
- [Canonical query bundle](#canonical-query-bundle)
- [Explorer fields to on-chain queries](#explorer-fields-to-on-chain-queries)
- [Proposal JSON shapes](#proposal-json-shapes)
- [Extract a software-upgrade plan](#extract-a-software-upgrade-plan)
- [Quorum, threshold, and veto](#quorum-threshold-and-veto)
- [Validator votes](#validator-votes)
- [Pagination](#pagination)
- [Governance transactions](#governance-transactions)
- [Upgrade module versus proposal content](#upgrade-module-versus-proposal-content)

## Select a network

For mainnet, select the default, non-archive RPC with the `ud` wrapper:

```bash
ud nibi cfg prod
nibiru config
```

Command `ud nibi cfg prod` also runs `nibiru config output json`, so the
examples here omit `-o json`. Use `ud nibi cfg prod --archive` only for
historical read-only queries the default RPC has pruned, then restore the
default with `ud nibi cfg prod`.

For testnet-2, pass flags instead of rewriting the shared config:

```bash
NET=(--node https://rpc.testnet-2.nibiru.fi:443 --chain-id nibiru-testnet-2 -o json)
nibiru q gov proposal 41 "${NET[@]}"
```

## Canonical query bundle

Run these first when asked about a proposal:

```bash
ID=27
nibiru q gov proposal "$ID"
nibiru q gov votes "$ID" --limit 1000
nibiru q gov tally "$ID"
nibiru q gov deposits "$ID"
nibiru q gov params

nibiru q staking pool
nibiru q staking validators --limit 2000
```

## Explorer fields to on-chain queries

| Explorer area | Command | JSON paths and notes |
| --- | --- | --- |
| Title, summary, status | `nibiru q gov proposal "$ID"` | `.title`, `.summary`, `.status` |
| Voting start and end | `nibiru q gov proposal "$ID"` | `.voting_start_time`, `.voting_end_time` |
| Submit and deposit times | `nibiru q gov proposal "$ID"` | `.submit_time`, `.deposit_end_time` |
| Proposer | `nibiru q gov proposal "$ID"` or `nibiru q gov proposer "$ID"` | `.proposer` |
| Total deposit | `nibiru q gov proposal "$ID"` | `.total_deposit[]` |
| Deposits tab | `nibiru q gov deposits "$ID"` | `.deposits[]` |
| Votes tab | `nibiru q gov votes "$ID" --limit 1000` | `.votes[]`, with weighted `options[]` |
| Tally totals | `nibiru q gov tally "$ID"` | `.yes_count`, `.no_count`, `.abstain_count`, `.no_with_veto_count` |
| Quorum and threshold params | `nibiru q gov params` or `nibiru q gov param tallying` | `.tally_params.quorum`, `.threshold`, `.veto_threshold` |
| Voting and deposit params | `nibiru q gov params` or `nibiru q gov param voting\|deposit` | `voting_period`, `min_deposit`, `max_deposit_period` |
| Quorum denominator | `nibiru q staking pool` | `.bonded_tokens` |
| Validator set with power | `nibiru q staking validators --limit 2000` | `.validators[]` with `.tokens`, `.status`, `.operator_address` |
| Upgrade plan in a proposal | `nibiru q gov proposal "$ID"` | See [Extract a software-upgrade plan](#extract-a-software-upgrade-plan) |
| Scheduled upgrade plan | `nibiru q upgrade plan` | Empty until a proposal passes |

## Proposal JSON shapes

Nibiru shows software upgrades as a gov v1 proposal whose `messages[]` wraps
legacy content:

- `messages[].@type` is `/cosmos.gov.v1.MsgExecLegacyContent`.
- `messages[].content.@type` is
  `/cosmos.upgrade.v1beta1.SoftwareUpgradeProposal`.

The plan therefore lives under `.messages[] | .content.plan`. Older SDK
versions surface legacy proposals under a single `content` field (gov
v1beta1), so scripts that read history should handle both shapes.

## Extract a software-upgrade plan

Read `plan.name`, `plan.height`, and `plan.info`:

```bash
nibiru q gov proposal "$ID" | jq '
  .messages[]
  | select(."@type" == "/cosmos.gov.v1.MsgExecLegacyContent")
  | .content
  | select(."@type" == "/cosmos.upgrade.v1beta1.SoftwareUpgradeProposal")
  | .plan
'
```

Field `plan.info` is a JSON string. Nibiru plans carry a `binaries` object with
checksum-addressed `linux/amd64` and `linux/arm64` archive URLs and a `docker`
image. Parse it:

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

Check whether a plan name was already applied, and when:

```bash
nibiru q upgrade applied v2.21.0 | jq -r '.header.height'
```

The query returns the header of the block where the plan applied.

## Quorum, threshold, and veto

- **Quorum:** `(yes + no + abstain + veto) / bonded_tokens`
- **Threshold:** `yes / (yes + no + veto)`; abstain is excluded.
- **Veto:** `veto / (yes + no + veto)`; abstain is excluded.

Inputs come from three queries. Command `nibiru q gov tally` gives the vote
weights, command `nibiru q gov params` gives `quorum`, `threshold`, and
`veto_threshold`, and command `nibiru q staking pool` gives `bonded_tokens`.

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

Field meanings:

- `tally` and `bonded_tokens`: raw output of the tally and staking pool
  queries.
- `quorum_required_pct`, `threshold_required_pct`, and `veto_threshold_pct`:
  the `tally_params` decimal fractions converted to percent.
- `total_voted_tokens`: `yes + no + abstain + veto`.
- `quorum_pct`: `total_voted_tokens / bonded_tokens * 100`.
- `yes_pct_of_non_abstain` and `veto_pct_of_non_abstain`: computed over
  `yes + no + veto`.

Abstain counts toward quorum but not toward the threshold or veto
percentages, which matches governance semantics and typical explorer
displays.

## Validator votes

Command `nibiru q gov votes` lists every voter, including delegators. To match
an explorer's "Validators Votes" view, convert each validator's
`nibivaloper...` operator address to its `nibi...` account address with
command `nibiru debug addr`, which prints `Bech32 Acc:` and `Bech32 Val:`
lines.

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

Pitfalls:

- Command `nibiru q staking validators` includes unbonded and unbonding
  validators; explorers usually show only the bonded set.
- Governance voting power is delegation-weighted. A validator's `tokens` is a
  proxy for its voting power, not a per-voter vote weight. One account that
  delegates to several validators carries all of that stake in one vote.

## Pagination

List queries accept some of `--limit`, `--page`, `--offset`, `--page-key`,
`--count-total`, and `--reverse`:

```bash
nibiru q gov votes "$ID" --limit 100 --page 2
nibiru q gov deposits "$ID" --limit 100 --page 1
nibiru q gov proposals --limit 5 --reverse
```

## Governance transactions

Select the network's default, non-archive RPC before any governance
transaction. Never broadcast through an archive RPC.

```bash
# Vote
nibiru tx gov vote "$ID" yes --from <key-or-addr>

# Weighted vote
nibiru tx gov weighted-vote "$ID" yes=0.6,no=0.3,abstain=0.1 --from <key-or-addr>

# Deposit
nibiru tx gov deposit "$ID" 1000000unibi --from <key-or-addr>
```

A vote without `--gas` can run out of gas; `--gas 200000` has worked on
mainnet and testnet. A vote can be changed until voting ends by voting again.

Command `nibiru tx gov submit-proposal` takes a JSON file of `messages`,
metadata, and deposit; see `nibiru tx gov submit-proposal --help`. Software
upgrades use command `nibiru tx gov submit-legacy-proposal software-upgrade`.

## Upgrade module versus proposal content

- Command `nibiru q gov proposal "$ID"` shows what is being voted on,
  including the upgrade plan embedded in the proposal messages.
- Command `nibiru q upgrade plan` shows the plan scheduled in module
  `x/upgrade`. Until the proposal passes, it returns
  `Error: no upgrade scheduled`.

While voting is open, treat the proposal as the source of truth.

## Links

- Nibiru governance lifecycle: <https://nibiru.fi/docs/community/governance.html>
- Submitting proposals: <https://nibiru.fi/docs/community/submitting-proposals.html>
- Cosmos SDK upgrade module: <https://docs.cosmos.network/v0.53/build/modules/upgrade>
