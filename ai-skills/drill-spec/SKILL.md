---
name: drill-spec
description: Interview the user relentlessly about a plan, spec, or design until reaching shared understanding, resolving each branch of the decision tree. Ask up to five direct questions per turn; ask fewer only when one answer blocks the next useful question. Use when user wants to stress-test a plan, get grilled on their design, or mentions "drill the spec" or "drill the plan".
---

Interview me about a plan, spec, or design until we reach shared understanding.
Ask direct questions that resolve concrete decisions, ambiguities, risks, and edge
cases. For each question, provide your recommended answer.

1. Ask up to five questions per turn. Ask fewer when a single answer blocks the
   next useful question. Ask in the chat, not with the question tool.

2. If a question can be answered by exploring the codebase, explore the codebase
   instead.

3. Record decisions as the discussion progresses. If the user is working from an
   existing spec, plan, issue, or design document, write resolved answers back
   into that document iteratively. If there is no obvious document, create or
   propose a short notes/spec file.

4. When recording implementation decisions, add or update one or more top-level
   `## Impl` sections. Use concise logical flows where they make the design
   easier to execute:

   ```text
   input or trigger
     -> state read/write
     -> validation
     -> result
   ```

5. When recording an answer, capture the decision, rationale, relevant codebase
   or operational context, and important caveats. Use open checkbox sub-bullets
   only for unresolved work, decisions, or tests. A resolved decision may remain
   as a completed decision task or become settled prose, following agent skill
   `md-tasks`.

6. Continue drilling while material design ambiguity remains for the
   implementation sequence being handed off. Answered questions alone do not
   make that scope ready for implementation. When the design stabilizes:
   - Follow agent skill `md-tasks` for a final normalization pass.
   - Preserve settled decisions and rationale while deriving separate open
     implementation, validation, rollout, or deferred tasks.
   - Check that no actionable consequence remains hidden in narrative prose.
   - If the artifact is an epic, follow agent skill `epics` for placement,
     structure, and handoff synthesis, including multiple top-level `## Impl`
     sections when they help separate implementation sequences.
   - Explicitly defer non-blocking questions so one stable `## Impl` sequence
     can proceed without implying that every later sequence is fully specified.
