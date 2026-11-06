import { parseWaitSeconds, rpcErrorText } from "./telemetry/middleware"

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
    }
  | {
      /** Operation completed successfully. */
      type: "action_success"
      /** Telegram chat ID or peer for the successful action. */
      chatId: number | string
      /** Stable action name, for example `owner_target_add`. */
      action: string
      /** Elapsed milliseconds from start to success, flood sleeps included. */
      elapsedMs: number
    }
  | {
      /** Operation failed; mtcute already retried any short flood wait. */
      type: "action_error"
      /** Telegram chat ID or peer for the failed action. */
      chatId: number | string
      /** Stable action name, for example `transfer_owner`. */
      action: string
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
  })
  try {
    const result = await operation()
    await deps.logger.event({
      type: "action_success",
      chatId: context.chatId,
      action: context.action,
      elapsedMs: deps.now() - startedAt,
    })
    return result
  } catch (error) {
    await deps.logger.event({
      type: "action_error",
      chatId: context.chatId,
      action: context.action,
      errorName: errorName(error),
      errorMessage: errorMessage(error),
    })
    throw error
  }
}

/** Why a batch stopped early on a Telegram rate limit. */
export interface FloodStop {
  /** Telegram error code, e.g. `FLOOD_WAIT_15382` or `PEER_FLOOD`. */
  code: string
  /** Wait Telegram asked for; `null` for `PEER_FLOOD`. */
  waitS: number | null
  /** When the wait ends (ISO); `null` when Telegram gives no time. */
  resumeAt: string | null
}

/**
 * Returns a stop when `error` means the rest of a batch would only hit the
 * same limit: any `PEER_FLOOD`, or a `FLOOD_WAIT_X` (and premium/test-phone
 * variants) longer than `maxWaitMs`, which mtcute did not sleep through.
 * `SLOWMODE_WAIT_X` is per chat, so it returns `null` and the batch goes on.
 */
export function floodStopFor(
  error: unknown,
  maxWaitMs: number,
  nowMs: number,
): FloodStop | null {
  const code = rpcErrorText(error)
  if (code === null) return null
  if (code === "PEER_FLOOD") return { code, waitS: null, resumeAt: null }
  if (code.startsWith("SLOWMODE_WAIT_")) return null
  const waitS = parseWaitSeconds(code)
  if (waitS === null || waitS * 1000 <= maxWaitMs) return null
  return {
    code,
    waitS,
    resumeAt: new Date(nowMs + waitS * 1000).toISOString(),
  }
}

/** A flood stop plus where in the batch it happened. */
export interface BatchStop<T> extends FloodStop {
  item: T
  /** Zero-based index of the item that hit the limit. */
  index: number
  /** Items after it that were not started. */
  remaining: number
}

export interface ChatBatchOptions<T> {
  /** Runs one item; a throw is reported to `onItemError`. */
  processItem: (item: T, index: number) => Promise<void>
  /** Records a per-item failure; the batch continues unless it stops. */
  onItemError: (item: T, error: unknown, index: number) => void | Promise<void>
  /** Runs between items (e.g. the inter-chat delay), not after the last. */
  betweenItems?: (item: T, index: number) => void | Promise<void>
  /** Flood waits longer than this stop the batch. */
  maxWaitMs: number
  now?: () => number
}

/**
 * Processes `items` in order, isolating per-item errors, and stops at the
 * first error `floodStopFor` treats as batch-wide.
 */
export async function runChatBatch<T>(
  items: readonly T[],
  options: ChatBatchOptions<T>,
): Promise<{ processed: number; floodStop: BatchStop<T> | null }> {
  const now = options.now ?? Date.now
  let processed = 0
  for (const [index, item] of items.entries()) {
    processed += 1
    try {
      await options.processItem(item, index)
    } catch (error) {
      await options.onItemError(item, error, index)
      const stop = floodStopFor(error, options.maxWaitMs, now())
      if (stop) {
        return {
          processed,
          floodStop: {
            ...stop,
            item,
            index,
            remaining: items.length - index - 1,
          },
        }
      }
    }
    if (index < items.length - 1) await options.betweenItems?.(item, index)
  }
  return { processed, floodStop: null }
}
