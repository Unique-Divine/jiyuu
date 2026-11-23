# Request a Deal Chat

Use this procedure to guide a teammate who has permission to request chats.
Telegram checks the requesting account against the team's requester
allowlist. A Deal Desk reader credential does not grant access to the bot.

1. Ask the teammate to open a private conversation with
   [SaiTeam_bot](https://t.me/SaiTeam_bot).
2. Have them send `/newdealchat @TRADER` with the trader's actual Telegram
   handle. To supply a display name for the chat title, put it after the
   handle, as in `/newdealchat @example_atlas Example Atlas`.
3. Wait for the bot's reply with the group and invite links. The bot assigns
   a ready chat from the team's pool. Share the invite only with the intended
   participant. Check the join status before assuming they joined.
4. If the invite needs to be replaced, DM the bot with `/chatlink` for the
   most recent claimed chat. Use `/chatlink CHAT_ID` for a specific claimed
   chat. Replace `CHAT_ID` with its actual ID.

If the pool is empty or the bot reports an unresolved setup, follow its reply
or bring the request to the operator. Keep the existing request context for
the handoff. This procedure is for a human requester. Return the steps to
the teammate so they can run them from their own authorized Telegram account.
