---
name: drill-spec
description: Interview the user relentlessly about a plan, spec, or design until reaching shared understanding, resolving each branch of the decision tree. Use when user wants to stress-test a plan, get grilled on their design, or mentions "drill the spec" or "drill the plan".
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

4. When recording implementation decisions, add or update a `## Impl` section.
   Use concise logical flows where they make the design easier to execute:

   ```text
   input or trigger
     -> state read/write
     -> validation
     -> result
   ```

5. When recording an answer, capture the decision, rationale, relevant codebase
   or operational context, and important caveats. Use checkbox sub-bullets only
   for unresolved work, decisions, or tests.
