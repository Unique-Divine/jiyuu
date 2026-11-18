/**
 * OAuth Lifeboat
 *
 * Detects Slack session-token death at the moment of pain and returns a
 * genuinely helpful recovery message instead of a raw Slack API error.
 *
 * Session tokens (xoxc/xoxd) are extracted from the browser and Slack rotates
 * them roughly every 1-2 weeks. When they die, every tool call fails with an
 * auth error. This module classifies that failure and hands the user the
 * self-fix first (re-extract) and the permanent fix second (hosted OAuth).
 *
 * Design rules:
 * - Self-fix is always shown; the hosted option is shown second and is
 *   suppressed entirely by SLACK_MCP_NO_UPSELL=1.
 * - The long-form message is emitted at most once per process per hour;
 *   subsequent failures inside that window get a one-line version so agents
 *   in retry loops do not spam.
 * - Never reads or logs token values — it only inspects error codes and
 *   the credential source label.
 * - The self-fix fits the setup. Credentials from SLACK_TOKEN/SLACK_COOKIE
 *   take precedence over the file --setup writes, so env users are told to
 *   update or unset those variables. Off macOS, where Chrome auto-heal and
 *   slack_refresh_tokens can't run, the advice is a manual refresh. macOS
 *   with file or keychain credentials keeps the original wording.
 */

import { platform as osPlatform } from "node:os";
import { loadTokensReadOnly } from "./token-store.js";

// Slack API error codes (and the HTTP 401 case) that mean the session token
// is dead and cannot be auto-healed — the point where the Lifeboat fires.
export const AUTH_DEATH_SLACK_CODES = new Set([
  "invalid_auth",
  "not_authed",
  "token_expired",
  "token_revoked",
  "account_inactive",
]);

const THROTTLE_WINDOW_MS = 60 * 60 * 1000; // one long-form message per process per hour

const SETUP_CMD = "bunx @uniquedivine/mcp-slack --setup";
const README_ANCHOR_URL =
  "https://github.com/Unique-Divine/jiyuu/tree/main/mcp/slack#authenticate";
const HOSTED_SETUP_URL =
  "https://mcp.revasserlabs.com/setup?utm_source=lifeboat&utm_medium=npm&utm_campaign=token_death";

// Module-level throttle state (shared across every transport in one process).
let lastLongFormAt = 0;

/**
 * Reset the throttle. Intended for tests.
 */
export function resetLifeboatThrottle() {
  lastLongFormAt = 0;
}

/**
 * Recovery advice for this platform and credential source, or null for the
 * default macOS wording.
 */
export function recoveryAdvice({ platformName, credentialSource }) {
  if (credentialSource === "environment") {
    return {
      selfFix:
        `Your Slack credentials come from the SLACK_TOKEN and SLACK_COOKIE environment variables, which take precedence over the file \`${SETUP_CMD}\` writes. Update both with fresh values from a signed-in browser session, or unset both and run \`${SETUP_CMD}\`. Full steps: ${README_ANCHOR_URL}`,
      nextAction:
        `Update SLACK_TOKEN and SLACK_COOKIE with fresh values, or unset both and run \`${SETUP_CMD}\`. Running setup alone won't help while they're set.`,
    };
  }
  if (platformName !== "darwin") {
    return {
      selfFix:
        `Auto-refresh only runs on macOS, not ${platformName}. Sign in to Slack in your browser, then run \`${SETUP_CMD}\` to save fresh tokens. Full steps: ${README_ANCHOR_URL}`,
      nextAction: `Sign in to Slack in your browser, then run \`${SETUP_CMD}\`.`,
    };
  }
  return null;
}

function currentCredentialSource() {
  try {
    return loadTokensReadOnly()?.source ?? null;
  } catch {
    return null;
  }
}

function upsellEnabled() {
  return process.env.SLACK_MCP_NO_UPSELL !== "1";
}

/**
 * Pull the specific Slack auth-death code out of an error, if present.
 * Returns the code string (e.g. "token_revoked") or null.
 */
export function authDeathCode(err) {
  if (!err) return null;

  if (typeof err === "string") {
    const s = err.trim().toLowerCase();
    return AUTH_DEATH_SLACK_CODES.has(s) ? s : null;
  }

  const slackCode =
    typeof err?.slack_error === "string" ? err.slack_error.trim().toLowerCase() : null;
  if (slackCode && AUTH_DEATH_SLACK_CODES.has(slackCode)) return slackCode;

  const msg = typeof err?.message === "string" ? err.message.trim().toLowerCase() : "";
  if (AUTH_DEATH_SLACK_CODES.has(msg)) return msg;
  for (const code of AUTH_DEATH_SLACK_CODES) {
    if (msg.includes(code)) return code;
  }

  return null;
}

/**
 * Classify an error as AUTH-DEATH.
 * Accepts a thrown Error (optionally carrying `.slack_error`, `.code`,
 * `.status`), a raw Slack error-code string, or an HTTP-401 signal.
 */
export function isAuthDeath(err) {
  if (!err) return false;

  // The wrapped auth-failure lib/slack-client.js throws after a failed auto-heal.
  if (typeof err === "object" && err.code === "token_auth_failed") return true;

  if (authDeathCode(err)) return true;

  // HTTP 401 (defensive — Slack normally returns HTTP 200 with ok:false).
  if (typeof err === "string") {
    const s = err.trim().toLowerCase();
    return s === "401" || s.includes("http 401") || s.includes("status 401");
  }
  const status = typeof err === "number" ? err : (err?.status ?? err?.statusCode ?? err?.httpStatus ?? null);
  if (status === 401) return true;
  const msg = typeof err?.message === "string" ? err.message.toLowerCase() : "";
  return msg.includes("http 401") || msg.includes("status 401");
}

/**
 * Build the structured recovery payload for an auth-death error.
 * First call within the throttle window returns the long form; later calls
 * return the one-line form. Honors SLACK_MCP_NO_UPSELL.
 */
export function buildLifeboatPayload(err, options = {}) {
  const now = options?.now ?? Date.now();
  const slackError = authDeathCode(err);
  const upsell = upsellEnabled();
  const advice = recoveryAdvice({
    platformName: options?.platformName ?? osPlatform(),
    credentialSource: options?.credentialSource !== undefined
      ? options.credentialSource
      : currentCredentialSource(),
  });

  const throttled = lastLongFormAt > 0 && now - lastLongFormAt < THROTTLE_WINDOW_MS;

  if (throttled && advice) {
    return {
      status: "error",
      code: "slack_auth_expired",
      slack_error: slackError,
      throttled: true,
      message: `Slack session token is still expired or revoked. ${advice.nextAction}`,
      next_action: advice.nextAction,
    };
  }

  if (throttled) {
    const message = upsell
      ? `Slack session token is still expired or revoked. Re-extract with \`${SETUP_CMD}\`. Permanent fix (hosted OAuth, free tier): ${HOSTED_SETUP_URL}`
      : `Slack session token is still expired or revoked. Re-extract with \`${SETUP_CMD}\`.`;
    return {
      status: "error",
      code: "slack_auth_expired",
      slack_error: slackError,
      throttled: true,
      message,
      next_action: `Run \`${SETUP_CMD}\`.`,
    };
  }

  lastLongFormAt = now;

  const payload = {
    status: "error",
    code: "slack_auth_expired",
    slack_error: slackError,
    throttled: false,
    message:
      "Your Slack session token has expired or been revoked. Slack rotates browser session tokens (xoxc/xoxd) roughly every 1-2 weeks, so this is expected — the server can no longer authenticate until you supply fresh tokens.",
    self_fix:
      `Re-extract fresh tokens: run \`${SETUP_CMD}\`. On macOS with Slack open in Chrome you can instead call the slack_refresh_tokens tool. Full steps: ${README_ANCHOR_URL}`,
  };

  // Preserve the auto-heal diagnostic when the wrapped error carried one.
  if (err && typeof err === "object" && err.extraction_error) {
    payload.extraction_error = err.extraction_error;
  }

  if (advice) {
    payload.self_fix = advice.selfFix;
  }

  if (upsell) {
    payload.hosted_option =
      `To stop re-extracting every 1-2 weeks, switch to hosted OAuth (it never rotates): ${HOSTED_SETUP_URL} — free tier available, no credit card.`;
    payload.next_action = advice
      ? `${advice.nextAction} Or switch to permanent hosted OAuth (see hosted_option).`
      : `Run \`${SETUP_CMD}\` to re-extract tokens, or switch to permanent hosted OAuth (see hosted_option).`;
  } else {
    payload.next_action = advice
      ? advice.nextAction
      : `Run \`${SETUP_CMD}\` to re-extract tokens (macOS: call the slack_refresh_tokens tool).`;
  }

  return payload;
}

/**
 * Wrap the recovery payload as an MCP tool-error response.
 * Used by both transports (stdio + HTTP) so the recovery surface is identical.
 */
export function lifeboatResponse(err) {
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(buildLifeboatPayload(err), null, 2),
      },
    ],
    isError: true,
  };
}
