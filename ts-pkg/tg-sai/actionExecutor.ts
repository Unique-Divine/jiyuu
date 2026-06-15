/**
 * Structured event emitted by the batch action executor.
 *
 * These objects are the source of truth for both test assertions and production
 * JSONL logs. Keep event payloads stable enough that a failed or interrupted
 * operator run can be inspected after the fact.
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
      /** Telegram returned `FLOOD_WAIT_X` and the executor will retry later. */
      type: "flood_wait"
      /** Telegram chat ID or peer for the rate-limited action. */
      chatId: number | string
      /** Stable action name, for example `bot_promote_admin`. */
      action: string
      /** One-based attempt number that received the flood-wait error. */
      attempt: number
      /** Seconds Telegram requested in `FLOOD_WAIT_X`. */
      waitSeconds: number
      /** Milliseconds the executor sleeps, including configured buffer. */
      sleepMs: number
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
      /** Operation failed with a non-retryable error or exhausted retries. */
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
 * Runtime dependencies for `withFloodWaitRetry`.
 *
 * Passing these in keeps retry behavior deterministic in tests: sleep, time, and
 * logging can all be faked without importing mtcute or touching Telegram.
 */
export interface TelegramActionExecutorDeps {
  /** Event sink for action start, success, error, and flood-wait events. */
  logger: BatchLogger
  /** Sleep function used after Telegram returns `FLOOD_WAIT_X`. */
  sleep: (ms: number) => Promise<void>
  /** Clock function used to compute elapsed action time. */
  now: () => number
  /** Number of retries after the first failed attempt. */
  maxRetries: number
  /** Extra milliseconds added to Telegram's required flood-wait duration. */
  bufferMs: number
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
 * Runs one Telegram operation with `FLOOD_WAIT_X` retry handling.
 *
 * The operation is passed as a thunk so the executor can call the exact same
 * Telegram action after waiting. Non-flood errors and exhausted retry attempts
 * are logged as `action_error` and then rethrown for the batch runner to record
 * at the chat level.
 */
export async function withFloodWaitRetry<T>(
  context: TelegramActionContext,
  operation: () => Promise<T>,
  deps: TelegramActionExecutorDeps,
): Promise<T> {
  const startedAt = deps.now()
  const maxAttempts = deps.maxRetries + 1

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    await deps.logger.event({
      type: "action_start",
      chatId: context.chatId,
      action: context.action,
      attempt,
    })

    try {
      const result = await operation()
      await deps.logger.event({
        type: "action_success",
        chatId: context.chatId,
        action: context.action,
        attempts: attempt,
        elapsedMs: deps.now() - startedAt,
      })
      return result
    } catch (error) {
      const waitSeconds = parseFloodWaitSeconds(error)
      const canRetry = waitSeconds !== null && attempt < maxAttempts

      if (!canRetry) {
        await deps.logger.event({
          type: "action_error",
          chatId: context.chatId,
          action: context.action,
          attempt,
          errorName: errorName(error),
          errorMessage: errorMessage(error),
        })
        throw error
      }

      const sleepMs = waitSeconds * 1000 + deps.bufferMs
      await deps.logger.event({
        type: "flood_wait",
        chatId: context.chatId,
        action: context.action,
        attempt,
        waitSeconds,
        sleepMs,
      })
      await deps.sleep(sleepMs)
    }
  }

  throw new Error(`unreachable action executor state: ${context.action}`)
}
