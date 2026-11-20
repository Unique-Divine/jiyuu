# Prepare Daily Trader Follow-Ups

Use the [follow-up queries](../references/graphql.md#find-follow-ups) to find recorded payments and completed actions that may need a trader update. Use the user's stated review interval and deal list. If no interval is supplied, cover the previous seven complete UTC calendar days and state the exact window.

1. Query `PAYMENTS` and `ACTIONS` with `hasComms: false`. Narrow by `dealId` when the task names a deal. Filter payments by `paidAt` and actions by `completedAt` when present. If an event date is unknown, put the record in a separate review list. A record creation date describes when someone entered the record.
2. Check each candidate against the deal and its recorded execution evidence. Consult `COMMS` by deal and look for notes that cover the same event even when they lack a direct link. Drop candidates with sufficient evidence that the trader was told. Flag unclear matches or uncertain delivery for human review.
3. Report each remaining candidate with the deal ID, event type and date, record and evidence IDs, communication status, and a proposed next step. Use "no linked communication record" when the relationship is missing. Tell the reviewer what would establish delivery. Combine rows into a proposed follow-up only when they concern the same deal and the evidence supports doing so.
4. Summarize the dates, deals, and data sources covered. If a workspace-wide query hits the response limit, retry per deal and list any deals you could not check. If the task includes GitHub-only fixes, verify them separately in GitHub before adding them.

Return the queue to the teammate for review. Include suggested message text only if requested. A candidate can need a record lookup, a decision, or a draft, depending on the evidence.
