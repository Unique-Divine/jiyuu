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
 *
 * Text in Slack can carry injected instructions. To tie each write to a
 * message the agent actually read, the caller also passes the message's
 * current text, and messages older than a default 24-hour window are refused.
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
  session_changed: {
    message: "The Slack session changed after the ownership check, so the tool didn't make the change.",
    next_action: "Run slack_health_check to confirm the signed-in user, then call the tool again."
  },
  lookup_incomplete: {
    message: "The tool couldn't finish searching the thread for this message, so it didn't make the change.",
    next_action: "Check the timestamp with slack_get_thread and call the tool again."
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

// Default age window for edits and deletes. Env
// SLACK_MCP_EDIT_DELETE_MAX_AGE_HOURS overrides it.
export const DEFAULT_MAX_AGE_HOURS = 24;

// Safety cap on conversations.replies pages. The lookup narrows the call to
// the target ts, so it normally needs one page.
export const MAX_REPLY_PAGES = 50;

const SLACK_TS_PATTERN = /^\d+\.\d+$/;

/**
 * Build an invalid_argument error result for one input field.
 */
export function invalidArgument(field, message) {
  return messageWriteError("invalid_argument", { field }, {
    message,
    next_action: "Fix the argument and call the tool again."
  });
}

/**
 * Validate the arguments every message write shares: channel_id, timestamp,
 * and expected_text. Returns null when they are valid, otherwise an MCP
 * error result. Runs before any Slack call.
 */
export function validateTargetArgs(args) {
  const input = args && typeof args === "object" ? args : {};
  const { channel_id: channelId, timestamp, expected_text: expectedText } = input;
  if (typeof channelId !== "string" || channelId.trim() === "" || /\s/.test(channelId)) {
    return invalidArgument("channel_id", "channel_id must be a non-empty channel or DM ID with no spaces.");
  }
  if (typeof timestamp !== "string" || !SLACK_TS_PATTERN.test(timestamp)) {
    return invalidArgument("timestamp", "timestamp must be a Slack message timestamp such as \"1767368030.607599\".");
  }
  if (typeof expectedText !== "string") {
    return invalidArgument("expected_text", "expected_text must be the message's current text, as read from Slack.");
  }
  return null;
}

/**
 * Maximum message age, in hours, that an edit or delete may target.
 */
export function maxAgeHours(env = process.env) {
  const raw = env.SLACK_MCP_EDIT_DELETE_MAX_AGE_HOURS;
  const hours = Number(raw);
  return raw !== undefined && raw !== "" && Number.isFinite(hours) && hours > 0
    ? hours
    : DEFAULT_MAX_AGE_HOURS;
}

/**
 * The session token slackAPI would use right now, or "" when none is set.
 */
export function currentSessionToken() {
  return loadTokensReadOnly()?.token || "";
}

/**
 * Signed-in user ID from auth.test, cached per session token.
 */
export async function getSelfUserId(sessionToken = currentSessionToken()) {
  const cached = selfUserIdByToken.get(sessionToken);
  if (cached) return cached;

  const result = await slackAPI("auth.test", {}, { retryOnAuthFail: false });
  if (!result.user_id) {
    throw new Error("auth.test returned no user_id");
  }
  selfUserIdByToken.set(sessionToken, result.user_id);
  return result.user_id;
}

export function clearSelfUserIdCache() {
  selfUserIdByToken.clear();
}

/**
 * Fetch one message by channel and timestamp, or null when it doesn't exist.
 * Top-level messages come from conversations.history; thread replies only
 * appear in conversations.replies, which accepts a reply's own timestamp.
 * The replies call is narrowed to oldest = latest = ts (inclusive), so it
 * returns the parent and the target. It still follows next_cursor, and
 * throws Error("lookup_incomplete") if the page cap runs out first.
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

  let cursor;
  for (let page = 0; page < MAX_REPLY_PAGES; page += 1) {
    let replies;
    try {
      replies = await slackAPI("conversations.replies", {
        channel: channelId,
        ts: timestamp,
        oldest: timestamp,
        latest: timestamp,
        inclusive: true,
        limit: 200,
        cursor
      });
    } catch (error) {
      if (error?.message === "thread_not_found" || error?.message === "message_not_found") {
        return null;
      }
      throw error;
    }
    const found = (replies.messages || []).find((m) => m.ts === timestamp);
    if (found) return found;
    cursor = replies.response_metadata?.next_cursor;
    if (!cursor) return null;
  }
  throw new Error("lookup_incomplete");
}

/**
 * True when a re-read shows the message is no longer the user's live
 * message: absent, a "This message was deleted." tombstone, or not theirs.
 */
export function isGoneForSelf(message, selfId) {
  return !message || message.subtype === "tombstone" || message.user !== selfId;
}

/**
 * Confirm the target before a write:
 * 1. the message is no older than maxAgeHours() (checked before any call),
 * 2. it exists,
 * 3. the signed-in user wrote it,
 * 4. its text equals expected_text after trimming.
 *
 * Returns { ok: true, message, selfId, sessionToken } or
 * { ok: false, result } where result is a ready MCP error. Pass the result
 * to sessionChangedResult just before the write.
 */
export async function checkOwnMessage(channelId, timestamp, options = {}) {
  const { expectedText, now = Date.now(), env = process.env } = options;
  const details = { channel: channelId, timestamp };
  const sessionToken = currentSessionToken();

  const limitHours = maxAgeHours(env);
  const ageHours = (now - Number(timestamp) * 1000) / 3_600_000;
  if (ageHours > limitHours) {
    return {
      ok: false,
      result: messageWriteError("message_too_old", {
        ...details,
        age_hours: Math.round(ageHours * 10) / 10,
        max_age_hours: limitHours
      }, {
        message: `The message is older than the ${limitHours}-hour window for edits and deletes.`,
        next_action: "Leave it, or ask the user to raise SLACK_MCP_EDIT_DELETE_MAX_AGE_HOURS for this server."
      })
    };
  }

  let selfId;
  let message;
  try {
    [selfId, message] = await Promise.all([
      getSelfUserId(sessionToken),
      findMessage(channelId, timestamp)
    ]);
  } catch (error) {
    if (error?.message === "lookup_incomplete") {
      return { ok: false, result: messageWriteError("lookup_incomplete", details) };
    }
    throw error;
  }

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
  if (typeof expectedText !== "string" || (message.text ?? "").trim() !== expectedText.trim()) {
    return {
      ok: false,
      result: messageWriteError("text_mismatch", details, {
        message: "expected_text doesn't match the message's current text, so the tool won't change it.",
        next_action: "Read the message again with slack_conversations_history or slack_get_thread and confirm it's the one the user meant."
      })
    };
  }
  return { ok: true, message, selfId, sessionToken };
}

/**
 * Refuse the write when the session token changed after checkOwnMessage,
 * for example because slackAPI swapped in another Chrome session on
 * invalid_auth. Returns null when the session is unchanged.
 */
export function sessionChangedResult(ownership, details = {}) {
  if (currentSessionToken() === ownership.sessionToken) return null;
  clearSelfUserIdCache();
  return messageWriteError("session_changed", details);
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
