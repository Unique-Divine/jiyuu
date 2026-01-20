---
name: nibid-gov-upgrade
description: Queries Nibiru governance proposals (including software-upgrade proposals) using the `nibid` CLI, maps explorer UI fields to on-chain queries, computes quorum/threshold/veto metrics, and correlates validator votes using staking queries. Use when the user mentions `nibid q gov`, governance proposals, software upgrades, votes, deposits, tally, quorum/threshold, validators, proposal status, or `nibid tx gov`.
---

# Nibid Gov + Upgrade (Nibiru mainnet)

## Workflow: How to Use this Skill

Start with the canonical query bundle in
[`Quick start (mainnet)`](#quick-start-mainnet). That is the default first step
when the user asks about a proposal and you need the core governance state.

After that, choose the recipe that matches the user's question:

1. For a live proposal's voting properties, use
   [`Recipe: Show voting and quorum info for a live proposal`](#recipe-show-voting-and-quorum-info-for-a-live-proposal).
   This is the fast path when the user wants a voting and quorum summary instead
   of only raw `proposal` or `tally` JSON.
2. For governance thresholds in the same output, use
   [`reference.md#explorer-style-quorum-report`](./reference.md#explorer-style-quorum-report).
3. If the proposal is a software upgrade, use
   [`Software-upgrade proposal: extract plan (name/height/info/binaries)`](#software-upgrade-proposal-extract-plan-nameheightinfobinaries)
   to inspect `plan.name`, `plan.height`, and parsed binaries from `plan.info`.
4. If the user wants validator-only participation rather than all voters, use
   [`Validator votes (filter votes to validator set)`](#validator-votes-filter-votes-to-validator-set).

## Additional resources

- Quorum, threshold, and veto:
  [`reference.md#quorum-threshold-and-veto`](./reference.md#quorum-threshold-and-veto)
- Explorer-style quorum report:
  [`reference.md#explorer-style-quorum-report`](./reference.md#explorer-style-quorum-report)

## Quick start (mainnet)

1. Configure CLI for mainnet (uses your `ud` wrapper):

```bash
ud nibi cfg prod
nibid config
```

2. Pick a proposal id:

```bash
ID=27
```

3. Run the canonical set of queries (JSON output):

```bash
nibid q gov proposal "$ID"
nibid q gov votes "$ID" --limit 1000
nibid q gov tally "$ID"
nibid q gov deposits "$ID"
nibid q gov params

nibid q staking pool
nibid q staking validators --limit 1000
```

## Mapping: "Proposal #27" explorer fields → on-chain queries

- **Proposal metadata (status, times, title/summary, total_deposit, proposer)**:
  - `nibid q gov proposal "$ID"`
  - Example fields: `.status`, `.submit_time`, `.voting_end_time`, `.title`,
    `.summary`, `.total_deposit`, `.proposer`

- **Deposits table**:
  - `nibid q gov deposits "$ID"`

- **Votes table (all votes, incl. weighted)**:
  - `nibid q gov votes "$ID" --limit 1000`

- **Current tally (yes/no/veto/abstain counts)**:
  - `nibid q gov tally "$ID"`

- **Quorum/threshold/veto thresholds**:
  - `nibid q gov params` (or `nibid q gov param voting|deposit|tallying`)

- **Bonded voting power denominator (for quorum math)**:
  - `nibid q staking pool` (use `.bonded_tokens`)

## Software-upgrade proposal: extract `plan` (name/height/info/binaries)

Nibiru mainnet currently surfaces software upgrades as a gov v1 proposal that
executes legacy content (you'll typically see `MsgExecLegacyContent` wrapping a
`SoftwareUpgradeProposal`).

Extract the upgrade plan:

```bash
nibid q gov proposal "$ID" | jq '
  .messages[]
  | select(."@type" == "/cosmos.gov.v1.MsgExecLegacyContent")
  | .content
  | select(."@type" == "/cosmos.upgrade.v1beta1.SoftwareUpgradeProposal")
  | .plan
'
```

Parse the `binaries` object out of `plan.info` (it's a JSON string):

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

## Recipe: Show voting and quorum info for a live proposal

Quick command:  

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
    }
' <(nibid q gov tally "$ID") <(nibid q staking pool)
```

Raw sources:

- `nibid q gov tally "$ID"` for current vote weights
- `nibid q gov params` for quorum / threshold / veto parameters
- `nibid q staking pool` for `bonded_tokens`

For a fuller report with governance thresholds and field notes, see
[`reference.md#explorer-style-quorum-report`](./reference.md#explorer-style-quorum-report)
and [`reference.md#field-meanings`](./reference.md#field-meanings).

## Validator votes (filter votes to validator set)

`nibid q gov votes` includes **all** voters (delegators + validators). To match
"Validators Votes" UIs, filter to validator accounts by converting each
validator's `nibivaloper...` to its corresponding `nibi...` address.

Helpers:

```bash
# Shows both Bech32 Acc (nibi...) and Bech32 Val (nibivaloper...)
nibid debug addr nibi1ah8gqrtjllhc5ld4rxgl4uglvwl93ag0sh6e6v
```

Example report (bonded validators who voted on `$ID`):

```bash
ID=27
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

## Governance transactions (tx)

- **Vote**:

```bash
nibid tx gov vote "$ID" yes --from <key-or-addr>
```

- **Weighted vote**:

```bash
nibid tx gov weighted-vote "$ID" yes=0.6,no=0.3,abstain=0.1 --from <key-or-addr>
```

- **Deposit**:

```bash
nibid tx gov deposit "$ID" 1000000unibi --from <key-or-addr>
```

- **Submit a software-upgrade proposal (legacy content path)**:

```bash
nibid tx gov submit-legacy-proposal software-upgrade v2.11.0 \
  --title "v2.11.0" \
  --description "Upgrade to v2.11.0" \
  --deposit 20000000000unibi \
  --upgrade-height 36757700 \
  --upgrade-info '{"binaries":{"linux/amd64":"https://...","linux/arm64":"https://...","docker":"ghcr.io/...:2.11.0"}}' \
  --from <key-or-addr>
```

## Upgrade module queries (after a proposal passes)

- `nibid q upgrade plan` only works once an upgrade is scheduled in `x/upgrade`.
  Before then, it can return "no upgrade scheduled".