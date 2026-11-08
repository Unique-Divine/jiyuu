---
name: drill-spec
description: Interview the user relentlessly about a plan, spec, or design until you both reach shared understanding, resolving each branch of the decision tree. Ask up to five direct questions per turn that cover the core decisions and tradeoffs. Ask only questions for which the answer unblocks the *next* useful question. Use when user wants to stress-test a plan, refine and improve their design, or mentions "drill the spec" or "drill the plan".
---

Interview me about a plan, spec, or design until we reach shared understanding.
Ask direct questions that resolve concrete decisions, ambiguities, risks, and edge
cases. For each question, provide your recommended answer.

1. Ask up to five questions per turn. Ask fewer when a single answer blocks the
   next useful question. Ask in the chat, not with the question tool.

2. If a question can be answered by exploring the codebase, explore the codebase
   instead. If the conversation or an existing document already answers the
   question, use that answer instead of asking again.

3. Record decisions as the discussion progresses. If the user is working from an
   existing spec, plan, issue, or design document, write resolved answers back
   into that document iteratively. If there is no obvious document, create or
   propose a short notes/spec file. Follow agent skill `md-tasks` whenever the
   document contains tasks.

4. Put implementation-bearing decisions in numbered top-level sections:
   `## Impl 1: <Sentence>`, `## Impl 2: <Sentence case>`, and so on. Start with `## Impl 1` even
   when the document has one implementation sequence. The numbers make the
   document easy to navigate. They do not impose delivery order. Use concise
   logical flows where they make the design easier to execute:

   ```text
   input or trigger
     -> state read/write A
     -> other state change B happens
     -> validation
     -> result
   ```

5. When recording an answer, capture the decision, rationale, relevant codebase
   or operational context, and important caveats. 
   - Write a resolved decision as ordinary prose under its relevant `## Impl N`
   section. It's crucial not only to capture the decision but also take notes on
   the rationale and nuance behind the decision. Future agents won't have context
   reading only the final outcome.
   - Put each actionable consequence directly beneath it as an unchecked
   implementation, validation, rollout, communication, or deferred task. 
   - Never mark a decision `[x]` just because the user made it. Mark `[x]` only
   after the work itself is complete and evidence is recorded.

   ```markdown
   ## Impl 1: Example Impl section (Sentence case after the ":")

   Clear and logical description of the goal for this impl section. The goal may
   change as a result of drill-spec iterations. This skill is for problem solving
   for product, engineering, and business outcomes.

   - [ ] Use the X tool for logging with jsonl structured log lines. Done when
   there we've done any type of run live to demonstrate the logs are being
   emitted properly. We want jsonl because ...
   - [ ] Save golden data to use as fixtures from each of the core API or MCP
   calls so that we can add type safety guarantees and test coverage for
   logic that composes them. Using this pattern will mean we get the live data
   robustness guarantees while avoiding heavy HTTP calls and resultant flakiness. 
   We'll retain test determinism.
   ```

   ↑ I'm using examples of my real preferences, btw.

6. Continue drilling while material design ambiguity remains for the
   implementation sequence being handed off. Answered questions alone do not
   make that scope ready for implementation. When the design stabilizes:
   - Follow agent skill `md-tasks` for a final normalization pass.
   - Preserve settled decisions and rationale while deriving separate open
     implementation, validation, rollout, or deferred tasks.
   - Check that no actionable consequence remains hidden in narrative prose.
   - If the artifact is an epic, follow agent skill `epics` for placement,
     structure, and handoff synthesis. Keep the numbered `## Impl N` sections
     created during the drill, adding more when they improve navigation.
   - Explicitly defer non-blocking questions so one stable `## Impl N` sequence
     can proceed without implying that every later sequence is fully specified.
