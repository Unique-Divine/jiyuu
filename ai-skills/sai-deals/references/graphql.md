# Deal Desk GraphQL

Send a JSON HTTP `POST` to `https://ops-api.sai.fun/graphql` with
`Authorization: Bearer <issued-key>`. Load `SAI_DEAL_DESK_TOKEN` using
[Install and Connect](install.md#connect-the-agent).

## Check the Credential

In Bash, define the helper below. It passes the bearer header on stdin and
stops on an HTTP error or a GraphQL error. An HTTP 200 can still contain a
GraphQL error. A 401 usually means the key is absent, invalid, or revoked.

```bash
deal_desk_query() (
  set +x
  set -o pipefail
  : "${SAI_DEAL_DESK_TOKEN:?Load the Deal Desk key from your secret store}"
  case "$SAI_DEAL_DESK_TOKEN" in
    *$'\n'*|*$'\r'*) printf 'Deal Desk key contains a line break.\n' >&2; return 1 ;;
  esac
  printf 'Authorization: Bearer %s\n' "$SAI_DEAL_DESK_TOKEN" |
    curl --disable --silent --show-error --fail --max-time 30 --proto '=https' \
      --header @- --header 'Content-Type: application/json' \
      --data-binary "$1" https://ops-api.sai.fun/graphql |
    jq -e 'if (.errors // [] | length) > 0 then error("Deal Desk GraphQL query failed") else .data end'
)

deal_desk_query '{"query":"query { credential }"}' |
  jq -e '.credential | select(.permissions == ["records:read"] and .revoked == false) | {label, permissions}'
```

## Read a Deal Brief

The examples use IDs from the [synthetic deal exercise](../examples/atlas.md).
Before querying the hosted API, replace `EXAMPLE-ATLAS` with a live selector,
then use the deal and communications IDs returned by your queries in place
of the fixture IDs.

Use a referral code, stable deal ID, or EVM or Nibiru wallet address. When
starting with a trader name, use directory lookup or [Ops](https://sai.fun/ops)
to identify a selector. GraphQL field `view` returns JSON, with columns
selected by its `fields` argument.

```bash
selector='EXAMPLE-ATLAS'
view="$(deal_desk_query "$(jq -nc --arg selector "$selector" '{
  query: "query DealBrief($selector: String!) { view(selectors: $selector, fields: \"deal,directory,matchedSelectors,signals,latestEvaluations,missingEvaluationSignalIds,payments,completions,actions,note\") }",
  variables: {selector: $selector}
}')")" || exit
printf '%s\n' "$view" | jq '.view | {deals: [.deals[].deal.id], errors}'
```

The variable `view` holds the full result. Check `view.errors` for projection
errors and `view.deals` for matches. If a wallet resolves to multiple deals,
show each stable ID and its matched selectors, then ask which deal to review.
Use that stable ID in comms queries.

## Read Communications

GraphQL field `commsRecords` returns completed notes and communication
workflow records for the selected deal. The example requests two records so
pagination is visible. Use a page size from 1 to 100. Pass
`pageInfo.endCursor` as `after` while `pageInfo.hasNextPage` is true.

```bash
deal_id='deal-example-atlas'
after=''
page="$(deal_desk_query "$(jq -nc --arg deal "$deal_id" --arg after "$after" '{
  query: "query CommsPage($deal: String!, $after: String) { commsRecords(dealId: $deal, first: 2, after: $after) { nodes { id sourceId recordType status reviewReason platform chatId bodyMarkdown notes createdAt updatedAt deliveredAt paymentIds actionIds dealIds } pageInfo { endCursor hasNextPage } } }",
  variables: {deal: $deal, after: (if $after == "" then null else $after end)}
}')")" || exit
printf '%s\n' "$page" | jq '.commsRecords'
```

Run the preceding query again with `after` set to the returned cursor,
keeping the deal and query fixed. Cursors belong to one deal. A page reflects
stored records at query time, so start over from the first page for a fresh
view.

To inspect delivery attempts and review history, copy a reader `id` from
the list and use the same deal ID. Reader IDs are opaque. Keep `sourceId`
alongside `id` when citing the underlying record.

```bash
comms_id='completed_note:comms-atlas-paid'
detail="$(deal_desk_query "$(jq -nc --arg deal "$deal_id" --arg id "$comms_id" '{
  query: "query CommsDetail($deal: String!, $id: ID!) { commsRecord(dealId: $deal, id: $id) { id sourceId recordType status reviewReason platform chatId bodyMarkdown notes createdAt updatedAt deliveredAt paymentIds actionIds dealIds history { id kind actor commentMarkdown createdAt } historyTruncated deliveryEvidence { id status chatId messageId deliveredAt createdAt updatedAt } deliveryEvidenceTruncated } }",
  variables: {deal: $deal, id: $id}
}')")" || exit
printf '%s\n' "$detail" | jq '.commsRecord'
```

A detail result can be null when the record is missing, belongs to another
deal, or has been superseded by its completed note. Refresh the list for the
same deal. Check `historyTruncated` and `deliveryEvidenceTruncated` when your
conclusion depends on older attempts. Use [Evidence](evidence.md#communications)
to interpret record types, dates, and links.

## Find Follow-Ups

Read payments and actions, then filter by the requested event dates. Use a
deal-scoped communications read to verify delivery for each candidate.
The `hasComms` filter tests whether a record has a direct communications
link. Omit it so linked notes with uncertain delivery stay in scope.

```bash
deal_desk_query '{"query":"query FollowUps { payments: records(input: {entity: PAYMENTS}) actions: records(input: {entity: ACTIONS}) }"}'
```

Add a stable `dealId` to each input for a deal-scoped review. A response-limit
error leaves the query incomplete. Continue with bounded deal-scoped reads
and name the deals you could cover. See [Daily Follow-Ups](../workflows/daily-follow-ups.md).
