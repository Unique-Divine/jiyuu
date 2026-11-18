# Roadmap

Candidate features for `@uniquedivine/mcp-slack`. Open an issue in
[Unique-Divine/jiyuu](https://github.com/Unique-Divine/jiyuu/issues) to
propose one.

## Shipped in 4.5

- [x] `slack_delete_message` and `slack_delete_message_unsafe`, which wrap
  `chat.delete` for your own messages
  ([jiyuu#128](https://github.com/Unique-Divine/jiyuu/issues/128)).
- [x] `slack_update_message` and `slack_update_message_unsafe`, which wrap
  `chat.update` for your own messages
  ([jiyuu#133](https://github.com/Unique-Divine/jiyuu/issues/133)).

## Candidates

| Feature | Effort | Why |
|---|---|---|
| Cursor passthrough for `conversations_history`, `replies`, `search`, and `unreads` | S | The tools take no cursor, so these reads return one page. Only `slack_get_full_conversation` pages through history. |
| `slack_schedule_message` | S to M | Wraps `chat.scheduleMessage`, so an agent can queue a message for later. |
| `slack_upload_file` | M | Wraps `files.getUploadURLExternal` and `files.completeUploadExternal`. The legacy `files.upload` method is deprecated. |

## Deferred

These need more work than they return, or Slack may block them for session
tokens:

- fetches that don't mark messages as read;
- OAuth `xoxp` and `xoxb` token modes;
- canvas create, read, update, and delete;
- user group management;
- `conversations.create`, `invite`, and `join`.

Slack gates more of `reminders.*` and `canvases.*` behind bot tokens. Check
that a session token works before starting either.
