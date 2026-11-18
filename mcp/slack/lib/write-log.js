/**
 * Write Log
 *
 * One structured stderr line per edit or delete attempt, so changes to Slack
 * messages leave a local record beyond the agent's chat. The line carries
 * only identifiers and outcome codes: never tokens, cookies, or message text.
 *
 * Format:
 *   slack-mcp write {"tool":"slack_delete_message","channel":"D0123","timestamp":"1794916496.123799","status":"deleted","code":null,"age_hours":0.2}
 */

const LOG_PREFIX = "slack-mcp write";
const CHANNEL_PATTERN = /^[A-Z0-9]{1,32}$/;
const TS_PATTERN = /^\d{1,12}\.\d{1,9}$/;
const CODE_PATTERN = /^[a-z][a-z0-9_]{0,63}$/;

let sink = (line) => console.error(line);

/**
 * Replace where log lines go. Returns the previous sink so tests can restore
 * it.
 */
export function setWriteLogSink(next) {
  const previous = sink;
  sink = next;
  return previous;
}

// Arguments come from the agent, so log a field only when it has the shape
// of an ID; anything else could carry message text.
function safeChannel(value) {
  return typeof value === "string" && CHANNEL_PATTERN.test(value) ? value : null;
}

function safeTimestamp(value) {
  return typeof value === "string" && TS_PATTERN.test(value) ? value : null;
}

function safeCode(value) {
  return typeof value === "string" && CODE_PATTERN.test(value) ? value : null;
}

/**
 * Message age in hours, to one decimal, or null for an invalid timestamp.
 */
export function ageHours(timestamp, now = Date.now()) {
  const ts = safeTimestamp(timestamp);
  if (!ts) return null;
  return Math.round(((now - Number(ts) * 1000) / 3_600_000) * 10) / 10;
}

/**
 * Status and error code from a tool result. The result's text is JSON that
 * may include message text, so only the status and code fields are read.
 */
function outcomeOf(result) {
  try {
    const payload = JSON.parse(result?.content?.[0]?.text ?? "{}");
    return {
      status: safeCode(payload.status) ?? (result?.isError ? "error" : null),
      code: safeCode(payload.code)
    };
  } catch {
    return { status: result?.isError ? "error" : null, code: null };
  }
}

/**
 * Build the log line for one attempt. Pure, for tests.
 */
export function formatWriteLogLine({ tool, args, status, code, now = Date.now() }) {
  const entry = {
    tool: safeCode(tool),
    channel: safeChannel(args?.channel_id),
    timestamp: safeTimestamp(args?.timestamp),
    status: safeCode(status),
    code: safeCode(code),
    age_hours: ageHours(args?.timestamp, now)
  };
  return `${LOG_PREFIX} ${JSON.stringify(entry)}`;
}

/**
 * Run a write handler and log one line for the attempt, whether it returns a
 * result or throws.
 */
export async function withWriteLog(tool, args, run) {
  let result;
  try {
    result = await run();
  } catch (error) {
    // A thrown Slack error's message is its code, such as invalid_auth.
    logSafely({ tool, args, status: "thrown", code: error?.message });
    throw error;
  }
  logSafely({ tool, args, ...outcomeOf(result) });
  return result;
}

function logSafely(entry) {
  try {
    sink(formatWriteLogLine(entry));
  } catch {
    // Logging must never change the tool's outcome.
  }
}
