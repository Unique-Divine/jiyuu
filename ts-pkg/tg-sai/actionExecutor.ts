/**
 * Structured progress event emitted by the batch action executor.
 *
 * These events are on-screen CLI progress (and copied to the run log). They do
 * not retry: mtcute's flood waiter is the only retry layer, and every Telegram
 * request, attempt, and flood wait is recorded by telemetry instead.
 */
export type BatchActionEvent =
  | {
      /** Operation attempt started. */
      type: "action_start"
      /** Telegram chat ID or peer for the attempted action. */
      chatId: number | string
      /** Stable action name, for example `bot_add`. */
      action: string
      /** One-based attempt number for this action. */
      attempt: number
    }
  | {
      /** Operation completed successfully. */
      type: "action_success"
      /** Telegram chat ID or peer for the successful action. */
      chatId: number | string
      /** Stable action name, for example `owner_target_add`. */
      action: string
      /** Number of attempts required before success. */
      attempts: number
      /** Elapsed milliseconds from first attempt to success. */
      elapsedMs: number
    }
  | {
      /** Operation failed; mtcute already retried any short flood wait. */
      type: "action_error"
      /** Telegram chat ID or peer for the failed action. */
      chatId: number | string
      /** Stable action name, for example `transfer_owner`. */
      action: string
      /** One-based attempt number that produced the final error. */
      attempt: number
      /** Error name normalized for logs. */
      errorName: string
      /** Error message normalized for logs. */
      errorMessage: string
    }

/**
 * Receives structured batch action events.
 *
 * Production implementations can write events to stdout, a JSONL file, or both.
 * Tests can use `MemoryBatchLogger` to inspect event order without parsing text.
 */
export interface BatchLogger {
  event(event: BatchActionEvent): void | Promise<void>
}

/**
 * Identifies the Telegram action being executed for log and retry events.
 */
export interface TelegramActionContext {
  /** Telegram chat ID or resolvable chat peer for the action. */
  chatId: number | string
  /** Stable action name such as `bot_add` or `owner_target_promote_admin`. */
  action: string
}

/**
 * Runtime dependencies for `runTelegramAction`. Passing these in keeps event
 * output deterministic in tests.
 */
export interface TelegramActionExecutorDeps {
  /** Event sink for action start, success, and error events. */
  logger: BatchLogger
  /** Clock function used to compute elapsed action time. */
  now: () => number
}

/**
 * Test logger that stores every event in insertion order.
 */
export class MemoryBatchLogger implements BatchLogger {
  readonly events: BatchActionEvent[] = []

  /**
   * Appends an event to the in-memory event list.
   */
  event(event: BatchActionEvent): void {
    this.events.push(event)
  }
}

/**
 * Extracts the wait duration from Telegram-style `FLOOD_WAIT_X` errors.
 *
 * mtcute and related libraries may expose the Telegram error string in the error
 * name, message, or both. This helper checks both fields and returns `null` for
 * non-flood errors so callers can distinguish retryable rate limits from normal
 * failures.
 */
export function parseFloodWaitSeconds(error: unknown): number | null {
  if (!(error instanceof Error)) {
    return null
  }

  const haystack = `${error.name} ${error.message}`
  const match = haystack.match(/FLOOD_WAIT_(\d+)/)
  if (!match) {
    return null
  }

  return Number(match[1])
}

/**
 * Returns a stable display name for unknown thrown values.
 */
export function errorName(error: unknown): string {
  if (error instanceof Error && error.name) {
    return error.name
  }
  return "UnknownError"
}

/**
 * Returns a stable display message for unknown thrown values.
 */
export function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message
  }
  return String(error)
}

/**
 * Runs one Telegram operation once and reports start, success, or error.
 *
 * There is no retry here: short `FLOOD_WAIT_X` waits are slept through by
 * mtcute's flood waiter, and longer ones surface as errors. Errors are logged
 * as `action_error` and rethrown for the batch runner to record per chat.
 */
export async function runTelegramAction<T>(
  context: TelegramActionContext,
  operation: () => Promise<T>,
  deps: TelegramActionExecutorDeps,
): Promise<T> {
  const startedAt = deps.now()
  await deps.logger.event({
    type: "action_start",
    chatId: context.chatId,
    action: context.action,
    attempt: 1,
  })
  try {
    const result = await operation()
    await deps.logger.event({
      type: "action_success",
      chatId: context.chatId,
      action: context.action,
      attempts: 1,
      elapsedMs: deps.now() - startedAt,
    })
    return result
  } catch (error) {
    await deps.logger.event({
      type: "action_error",
      chatId: context.chatId,
      action: context.action,
      attempt: 1,
      errorName: errorName(error),
      errorMessage: errorMessage(error),
    })
    throw error
  }
}
