# Brief One Deal

Start with [Install and Connect](../references/install.md). Use the
[GraphQL examples](../references/graphql.md) or the configured reader CLI.

1. Resolve the code, wallet, or stable deal ID. Check lookup errors and matched
   selectors. If several deals match, show their IDs and ask which one to use.
   Group multiple aliases for one deal by its stable ID.
2. Read the accepted terms and the saved evaluation. Name its cutoff and the
   part of the requested period it covers. Tie payments and actions to the
   corresponding term when a link exists.
3. Read delivery details for the relevant communications. Check all relevant
   pages and inspect notes linked to the deal as well as individual events.
   Use the [evidence rules](../references/evidence.md) when dates, payment
   forms, and coverage differ.

For a live brief, replace `DEAL_SELECTOR` with a referral code, wallet, or
stable deal ID. Use the stable `DEAL_ID` returned by `deals view` and a
`RECORD_ID` from `comms list`.

```bash
sai-ops --reader health
sai-ops --reader deals view DEAL_SELECTOR
sai-ops --reader comms list --deal DEAL_ID --first 2
sai-ops --reader comms show --deal DEAL_ID RECORD_ID
```

When `pageInfo.hasNextPage` is true, keep the deal fixed and pass the returned
`pageInfo.endCursor` to `comms list --after CURSOR`.

Return a short brief with the stable deal ID, the agreement, what was measured,
what executed, what was communicated, and one next step. Give record IDs and
dates or cutoffs for material claims. Name any source, missing page, ambiguous
match, or coverage gap that limits the answer. For a practice run, use the
[synthetic deal](../examples/atlas.md).
