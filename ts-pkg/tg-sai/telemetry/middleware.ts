import { AsyncLocalStorage } from "node:async_hooks"
import { networkMiddlewares } from "@mtcute/bun"
import type {
  FloodWaiterOptions,
  RpcCallMiddleware,
  RpcCallMiddlewareContext,
} from "@mtcute/bun"

import { uuidv7 } from "./db.ts"
import type {
  TelemetryEvent,
  TelemetryOutcome,
  TelemetrySink,
  TelemetrySubsystem,
} from "./db.ts"
import { summarizeRequest, summarizeTl } from "./summarize.ts"

/** Identity stamped on every row a client writes. */
export interface TelemetryContext {
  runId: string
  subsystem: TelemetrySubsystem
  command: string | null
  accountId?: number | null
  profile?: string | null
  version: string
}

export type TelemetryContextProvider = () => TelemetryContext

/**
 * Telemetry handed to `createMtcuteRuntimeClient`. `floodWaiter` configures
 * mtcute's single flood-wait retry layer for this process.
 */
export interface ClientTelemetry {
  sink: TelemetrySink
  context: TelemetryContextProvider
  floodWaiter?: FloodWaiterOptions
}

export const CALL_EVENT = "tg.rpc.call"
export const ATTEMPT_EVENT = "tg.rpc.attempt"
export const FLOOD_WAIT_EVENT = "tg.flood.wait"

const jobStorage = new AsyncLocalStorage<string>()

/** Runs `fn` with `jobId` attached to every Telegram call it makes. */
export function withTelemetryJob<T>(jobId: string, fn: () => T): T {
  return jobStorage.run(jobId, fn)
}

/** Job ID of the enclosing `withTelemetryJob`, if any. */
export function currentTelemetryJobId(): string | null {
  return jobStorage.getStore() ?? null
}

interface RpcErrorValue {
  _: "mt_rpc_error"
  errorCode: number
  errorMessage: string
}

function isRpcErrorValue(value: unknown): value is RpcErrorValue {
  return (
    typeof value === "object" &&
    value !== null &&
    (value as { _?: unknown })._ === "mt_rpc_error"
  )
}

const WAIT_PATTERN =
  /^(?:FLOOD|SLOWMODE|FLOOD_TEST_PHONE|FLOOD_PREMIUM)_WAIT_(\d+)$/

/** Seconds from `FLOOD_WAIT_N`-style messages; `null` otherwise. */
export function parseWaitSeconds(
  message: string | null | undefined,
): number | null {
  const match = message?.match(WAIT_PATTERN)
  return match ? Number(match[1]) : null
}

const MAX_ERR_CHARS = 500

function errorText(error: unknown): string {
  let text: string
  try {
    text =
      error instanceof Error
        ? `${error.name}: ${error.message}`
        : String(error)
  } catch {
    text = "unprintable error"
  }
  return text.length > MAX_ERR_CHARS ? `${text.slice(0, MAX_ERR_CHARS)}…` : text
}

/** Classifies a thrown error from the transport or an aborted call. */
export function thrownOutcome(
  error: unknown,
  ctx?: RpcCallMiddlewareContext,
): TelemetryOutcome {
  const name = error instanceof Error ? error.name : ""
  if (name === "AbortError" || ctx?.params?.abortSignal?.aborted) {
    return "aborted"
  }
  if (/timeout/i.test(name) || /timed? ?out/i.test(errorText(error))) {
    return "timeout"
  }
  return "error"
}

interface ResultFields {
  outcome: TelemetryOutcome
  errorCode: number | null
  err: string | null
  waitS: number | null
}

function resultFields(result: unknown): ResultFields {
  if (isRpcErrorValue(result)) {
    return {
      outcome: "error",
      errorCode: typeof result.errorCode === "number" ? result.errorCode : null,
      err: String(result.errorMessage),
      waitS: parseWaitSeconds(result.errorMessage),
    }
  }
  return { outcome: "ok", errorCode: null, err: null, waitS: null }
}

function thrownFields(
  error: unknown,
  ctx: RpcCallMiddlewareContext,
): ResultFields {
  const code = (error as { code?: unknown } | null)?.code
  const err = errorText(error)
  return {
    outcome: thrownOutcome(error, ctx),
    errorCode: typeof code === "number" ? code : null,
    err,
    waitS: parseWaitSeconds(
      (error as { text?: unknown } | null)?.text as string | undefined,
    ),
  }
}

interface CallState {
  requestId: string
  method: string
  jobId: string | null
  identity: TelemetryContext | null
  attempts: number
}

/** Same as mtcute's default `onBeforeWait` warning, which we replace. */
const SILENT_METHODS = new Set([
  "upload.getFile",
  "upload.saveFilePart",
  "upload.saveBigFilePart",
  "updates.getDifference",
  "updates.getChannelDifference",
])

function defaultWaitWarning(
  ctx: RpcCallMiddlewareContext,
  seconds: number,
): void {
  if (seconds < 5 && SILENT_METHODS.has(ctx.request._)) return
  const log = (ctx.manager as unknown as {
    _log?: { warn?: (...args: unknown[]) => void }
  })._log
  log?.warn?.(
    "%s resulted in a flood wait, will retry in %d seconds",
    ctx.request._,
    seconds,
  )
}

/**
 * Builds the full network middleware list for a telemetry-enabled client:
 * `[outer, ...basic({ floodWaiter }), inner]`.
 *
 * - outer: one `tg.rpc.call` row when a call starts and one when it ends.
 * - inner: one `tg.rpc.attempt` row per request that reaches the wire.
 * - `onBeforeWait`: one `tg.flood.wait` row each time mtcute sleeps.
 *
 * Logging never throws and never changes the call's result.
 */
export function createTelemetryMiddlewares(
  sink: TelemetrySink,
  ctxProvider: TelemetryContextProvider,
  floodOpts: FloodWaiterOptions = {},
): RpcCallMiddleware[] {
  const calls = new WeakMap<object, CallState>()

  const record = (
    state: CallState | undefined,
    event: Omit<TelemetryEvent, "runId" | "subsystem">,
  ): void => {
    try {
      const identity = state?.identity ?? ctxProvider()
      sink.record({
        runId: identity.runId,
        subsystem: identity.subsystem,
        command: identity.command,
        accountId: identity.accountId ?? null,
        profile: identity.profile ?? null,
        version: identity.version,
        requestId: state?.requestId ?? null,
        jobId: state?.jobId ?? currentTelemetryJobId(),
        ...event,
      })
    } catch {
      // Telemetry must never affect a Telegram call.
    }
  }

  const outer: RpcCallMiddleware = async (ctx, next) => {
    let state: CallState | undefined
    try {
      let identity: TelemetryContext | null = null
      try {
        identity = ctxProvider()
      } catch {
        identity = null
      }
      state = {
        requestId: uuidv7(),
        method: String(ctx.request._),
        jobId: currentTelemetryJobId(),
        identity,
        attempts: 0,
      }
      calls.set(ctx, state)
      const live = state
      let req: unknown = null
      try {
        req = summarizeRequest(live.method, ctx.request)
      } catch {
        req = { _: live.method }
      }
      record(live, {
        event: CALL_EVENT,
        method: live.method,
        outcome: "started",
        req,
      })
    } catch {
      state = undefined
    }

    const startedAt = performance.now()
    let result: unknown
    try {
      result = await next(ctx)
    } catch (error) {
      try {
        record(state, {
          event: CALL_EVENT,
          method: state?.method ?? null,
          attempt: state?.attempts ?? null,
          durationMs: Math.round(performance.now() - startedAt),
          ...thrownFields(error, ctx),
        })
      } catch {
        // Ignore.
      }
      throw error
    }
    try {
      const fields = resultFields(result)
      // floodWaiter answers from its stored wait without sending anything.
      const synthetic = fields.outcome === "error" && state?.attempts === 0
      record(state, {
        event: CALL_EVENT,
        method: state?.method ?? null,
        attempt: state?.attempts ?? null,
        durationMs: Math.round(performance.now() - startedAt),
        ...fields,
        req: synthetic ? { synthetic: true } : null,
        res: fields.outcome === "ok" ? summarizeTl(result) : null,
      })
    } catch {
      // Ignore.
    }
    return result
  }

  const inner: RpcCallMiddleware = async (ctx, next) => {
    let state: CallState | undefined
    let attempt = 1
    try {
      state = calls.get(ctx)
      if (state) {
        state.attempts += 1
        attempt = state.attempts
      }
    } catch {
      state = undefined
    }
    const startedAt = performance.now()
    let result: unknown
    try {
      result = await next(ctx)
    } catch (error) {
      try {
        record(state, {
          event: ATTEMPT_EVENT,
          method: String(ctx.request?._),
          attempt,
          durationMs: Math.round(performance.now() - startedAt),
          ...thrownFields(error, ctx),
        })
      } catch {
        // Ignore.
      }
      throw error
    }
    try {
      record(state, {
        event: ATTEMPT_EVENT,
        method: String(ctx.request?._),
        attempt,
        durationMs: Math.round(performance.now() - startedAt),
        ...resultFields(result),
      })
    } catch {
      // Ignore.
    }
    return result
  }

  const onBeforeWait = (ctx: RpcCallMiddlewareContext, seconds: number) => {
    try {
      const state = calls.get(ctx)
      record(state, {
        event: FLOOD_WAIT_EVENT,
        method: String(ctx.request?._),
        attempt: state?.attempts ?? null,
        outcome: "error",
        errorCode: 420,
        err: `wait ${seconds}s before retry`,
        waitS: seconds,
      })
    } catch {
      // Ignore.
    }
    try {
      if (floodOpts.onBeforeWait) floodOpts.onBeforeWait(ctx, seconds)
      else defaultWaitWarning(ctx, seconds)
    } catch {
      // A caller hook must not break the retry either.
    }
  }

  return [
    outer,
    ...networkMiddlewares.basic({
      floodWaiter: { ...floodOpts, onBeforeWait },
    }),
    inner,
  ]
}

/** Same telemetry with the account and profile filled in. */
export function withTelemetryIdentity(
  telemetry: ClientTelemetry | undefined,
  identity: { accountId?: number | null; profile?: string | null },
): ClientTelemetry | undefined {
  if (!telemetry) return undefined
  return {
    ...telemetry,
    context: () => ({ ...telemetry.context(), ...identity }),
  }
}
