---
name: new-chat-prompt
description: Writes a ready-to-paste prompt for a fresh chat instead of completing the requested task directly. Use when the user asks to start a new chat, move work to a new chat, or says to write a prompt for tackling the task elsewhere.
---

# New Chat Prompt

When this skill applies, do not complete the underlying task.

Instead, write a concise prompt the user can paste into a new chat where the task will be handled there.

Include:
- The user's goal
- Any important context already known that seems crucial to the task at hand.
- Clear instructions for the next agent

Keep the prompt clear and cogent while avoiding extra explanation unless
the user asks for it.

Do not duplicate the content of other artifacts like plans, issues, or diffs.
Instead, reference them by path, name, or retrieval guidelines. 
For example, you can include `gh issue view "$num"` for some GitHub ticket rather than writing
out the issue body yourself. Or in the case of a file/dir, prefer saying the path
with a one-liner on its utility for the task rather than copying tons of text
from the path.
