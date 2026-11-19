# Review a Deal

Use this procedure for a trader brief, a status question before a reply, or a reconciliation of what was promised and what was delivered. Start with the [deal brief query](../references/graphql.md#read-a-deal-brief).

1. Resolve the referral code, stable deal ID, or EVM or Nibiru wallet address. Check `matchedSelectors` and `view.errors`. A wallet may match several deals, and several aliases may match the same deal. Ask the user to pick when the result is ambiguous. Group the records by stable deal ID so alias matches cannot duplicate events.
2. Read `signals` for the agreement. Record the specific condition, date window, denomination, and payment form. Check when a note or agreement was recorded before treating it as current. A historical promise or complaint is evidence of the discussion. A current obligation needs supporting terms or a current decision.
3. Read `latestEvaluations`, `missingEvaluationSignalIds`, and any recorded coverage. An evaluation is an observation with a cutoff. A missing evaluation, stale measurement, or incomplete payment history leaves the outcome unknown. A recorded zero with sufficient coverage is evidence of zero within that window.
4. Read `payments`, `completions`, and `actions` for what happened. Tie the event to the relevant term when the link exists. Cite a receipt, transaction hash, or recorded execution evidence where available. Distinguish an evaluation outcome, a completed action, a payment record, and evidence that the payment executed. Keep amounts as exact decimal strings and distinguish USDC trading credits, withdrawable Sai deposits, and cash payments.
5. Check `comms` before claiming a trader heard an update. Inspect both direct event links and deal-level notes. A draft or a record of uncertain delivery supports a limited claim. Say what the communication record establishes.

Return a short summary with the stable deal ID, followed by the agreement, evidence, and any open decision. For evidence, include the record ID, the recorded time or cutoff, and the supported conclusion. Flag a missing source or unresolved match rather than filling it in from an old summary.
