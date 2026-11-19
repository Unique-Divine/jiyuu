# Deal Desk GraphQL Reference

The endpoint is `https://deals-api.sai.fun/graphql`. Use HTTP `POST` with a JSON
body and an `Authorization: Bearer` header. Browser sign-in at `/ops` has its
own Google session. The bearer credential is valid across Deal Desk until revoked.

## Load and Check the Credential

Configure `SAI_DEAL_DESK_TOKEN` in the local agent's secret store or launch it
with the 1Password CLI. For 1Password, copy the field's secret reference into a
local environment file. The file should contain the reference URI.

```dotenv
SAI_DEAL_DESK_TOKEN=op://<vault>/<item>/<field>
```

Launch the local agent with `op run --env-file ./deal-desk.env -- <agent-command>`.
The agent process inherits `SAI_DEAL_DESK_TOKEN`. Keep the resolved value out of
the prompt and shell history. Use the same launch method for a shell if testing manually.

In Bash, define this helper with shell tracing disabled. `curl` reads the
authorization header from standard input. The request body contains the query and
its variables. The helper requires `curl` and `jq`.

```bash
deal_desk_query() (
  set +x
  set -o pipefail
  : "${SAI_DEAL_DESK_TOKEN:?Load the Deal Desk key from your secret store}"
  printf 'Authorization: Bearer %s\n' "$SAI_DEAL_DESK_TOKEN" |
    curl --silent --show-error --fail --max-time 30 \
      --header @- --header 'Content-Type: application/json' \
      --data-binary "$1" https://deals-api.sai.fun/graphql |
    jq -e 'if (.errors // [] | length) > 0 then error([.errors[].message] | join("; ")) else .data end'
)

deal_desk_query '{"query":"query { credential }"}' |
  jq -e '.credential | select(.permissions == ["records:read"] and .revoked == false) | {label, permissions}'
```

Continue when this returns the credential label and `["records:read"]`. HTTP
`401` means the credential is missing, invalid, or revoked. The helper rejects
HTTP failures and GraphQL errors, which can arrive with HTTP `200`.

## Read a Deal Brief

The `view` field returns JSON. Pass `fields` as a comma-separated string. Replace
`EXAMPLE-CODE` with the requested deal ID or referral code.

```bash
deal_desk_query '{"query":"query DealBrief($selector: String!) { view(selectors: $selector, fields: \"deal,directory,matchedSelectors,signals,latestEvaluations,missingEvaluationSignalIds,payments,completions,actions,comms,note\") }","variables":{"selector":"EXAMPLE-CODE"}}'
```

Use `view.deals` and `view.errors` to check whether the
requested deal resolved. The query `directory` can help identify a selector when
the user supplies only a name or alias.

## Find Follow-Ups

The `records` field accepts `RecordReadInput` and returns JSON. These two reads
select payments and actions with no directly linked communication record.

```bash
deal_desk_query '{"query":"query FollowUps { payments: records(input: {entity: PAYMENTS, hasComms: false}) actions: records(input: {entity: ACTIONS, hasComms: false}) }"}'
```

Add `dealId` to either input to narrow the results to the stable deal ID. Query
`records` with `entity: COMMS` and `dealId` to check notes for the deal. The
`paymentId` and `actionId` filters let you inspect links for a particular event.
Apply a requested date filter to the returned event dates. If a workspace-wide
read hits the response limit, use deal-scoped reads and report which deals you
covered. Use a deal brief to check terms, context, and execution evidence before
drafting a follow-up.
