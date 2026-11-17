/**
 * Message Ownership Guard
 *
 * Shared checks for tools that change an existing message
 * (slack_delete_message, slack_update_message).
 *
 * Slack lets a workspace owner or admin delete other people's messages with a
 * session token, in public channels and in private channels they belong to.
 * These tools must only change the signed-in user's own messages, so each one
 * fetches the target message first and refuses locally when its author is
 * someone else.
 */

import { slackAPI } from "./slack-client.js";
import { loadTokensReadOnly } from "./token-store.js";

// Signed-in user ID per session token, so a token swap re-runs auth.test.
const selfUserIdByToken = new Map();

/**
 * Slack errors from chat.delete and chat.update that have a clear meaning
 * for the caller. Anything else propagates to the transport.
 */
export const SLACK_WRITE_ERRORS = Object.freeze({
  cant_delete_message: {
    message: "Slack refused the delete: the signed-in user can't delete this message.",
    next_action: "Only delete messages you sent."
  },
  cant_update_message: {
    message: "Slack refused the edit: the signed-in user can't edit this message.",
    next_action: "Only edit messages you sent."
  },
  no_permission: {
    message: "The workspace doesn't let this session make that change.",
    next_action: "Ask a workspace admin, or leave the message as it is."
  },
  edit_window_closed: {
    message: "The workspace's edit window for this message has passed.",
    next_action: "Send a corrected message instead, or delete this one and send it again."
  },
  msg_too_long: {
    message: "The new text is longer than Slack's 4,000-character limit.",
    next_action: "Shorten the text and retry."
  },
  message_not_found: {
    message: "No message with that timestamp exists in this channel.",
    next_action: "Read the channel with slack_conversations_history or the thread with slack_get_thread to get the exact timestamp."
  }
});

/**
 * Build an MCP error result for a message write.
 */
export function messageWriteError(code, details = {}, overrides = {}) {
  const known = SLACK_WRITE_ERRORS[code] || {};
  return {
    content: [{
      type: "text",
      text: JSON.stringify({
        status: "error",
        code,
        message: overrides.message || known.message || code,
        next_action: overrides.next_action || known.next_action,
        ...details
      }, null, 2)
    }],
    isError: true
  };
}

/**
 * Map a thrown Slack error to an MCP error result, or return null when the
 * error has no mapping and should propagate (auth death, rate limits, etc.).
 */
export function slackWriteErrorResult(error, details = {}) {
  const code = error?.message;
  if (!Object.hasOwn(SLACK_WRITE_ERRORS, code)) return null;
  return messageWriteError(code, details);
}

/**
 * Signed-in user ID from auth.test, cached per session token.
 */
export async function getSelfUserId() {
  const cacheKey = loadTokensReadOnly()?.token || "";
  const cached = selfUserIdByToken.get(cacheKey);
  if (cached) return cached;

  const result = await slackAPI("auth.test", {});
  if (!result.user_id) {
    throw new Error("auth.test returned no user_id");
  }
  selfUserIdByToken.set(cacheKey, result.user_id);
  return result.user_id;
}

export function clearSelfUserIdCache() {
  selfUserIdByToken.clear();
}

/**
 * Fetch one message by channel and timestamp, or null when it doesn't exist.
 * Top-level messages come from conversations.history; thread replies only
 * appear in conversations.replies, which accepts a reply's own timestamp.
 */
export async function findMessage(channelId, timestamp) {
  const history = await slackAPI("conversations.history", {
    channel: channelId,
    latest: timestamp,
    inclusive: true,
    limit: 1
  });
  const top = (history.messages || []).find((m) => m.ts === timestamp);
  if (top) return top;

  let replies;
  try {
    replies = await slackAPI("conversations.replies", {
      channel: channelId,
      ts: timestamp,
      limit: 1000
    });
  } catch (error) {
    if (error?.message === "thread_not_found" || error?.message === "message_not_found") {
      return null;
    }
    throw error;
  }
  return (replies.messages || []).find((m) => m.ts === timestamp) || null;
}

/**
 * Confirm the message exists and the signed-in user wrote it.
 * Returns { ok: true, message } or { ok: false, result } where result is a
 * ready MCP error.
 */
export async function checkOwnMessage(channelId, timestamp) {
  const details = { channel: channelId, timestamp };
  const [selfId, message] = await Promise.all([
    getSelfUserId(),
    findMessage(channelId, timestamp)
  ]);

  if (!message) {
    return { ok: false, result: messageWriteError("message_not_found", details) };
  }
  if (message.user !== selfId) {
    return {
      ok: false,
      result: messageWriteError("not_own_message", {
        ...details,
        author: message.user || message.bot_id || null
      }, {
        message: "The message belongs to someone else. This tool only changes messages the signed-in user sent.",
        next_action: "Check channel_id and timestamp, and pick one of your own messages."
      })
    };
  }
  return { ok: true, message };
}

/**
 * A logger for slackAPI that records whether it retried after a network
 * error, so a later message_not_found can be read as "the first attempt
 * probably landed".
 */
export function createRetryTracker(base = console) {
  const tracker = {
    retried: false,
    logger: {
      ...base,
      error: (...args) => {
        if (String(args[0]).startsWith("Network error on")) {
          tracker.retried = true;
        }
        base.error?.(...args);
      }
    }
  };
  return tracker;
}
