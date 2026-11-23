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

For a configured CLI, run the following commands with the selected identifiers.

```bash
sai-ops --reader health
sai-ops --reader deals view EXAMPLE-ATLAS
sai-ops --reader comms list --deal deal-example-atlas --first 2
sai-ops --reader comms show --deal deal-example-atlas completed_note:comms-atlas-paid
```

When `pageInfo.hasNextPage` is true, keep the deal fixed and pass the returned
`pageInfo.endCursor` to `comms list --after CURSOR`. Copy the record ID from
that list before running `comms show`.

Return a short brief with the stable deal ID, the agreement, what was measured,
what executed, what was communicated, and one next step. Give record IDs and
dates or cutoffs for material claims. Name any source, missing page, ambiguous
match, or coverage gap that limits the answer. For a practice run, use the
[synthetic deal](../examples/atlas.md).
