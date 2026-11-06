import type { RpcCallMiddleware, RpcCallMiddlewareContext } from "@mtcute/bun"

import {
  HoldStoreUnavailableError,
  isFailClosedGroup,
  methodGroup,
  parseHoldError,
  remainingSeconds,
  type FloodHold,
  type HoldGroup,
  type HoldStore,
} from "./holds.ts"

export const HOLD_ERROR_CODE = 420
export const HOLD_ERROR_PATTERN = /^TG_SAI_HOLD_(\d+)$/

/** Seconds from a `TG_SAI_HOLD_<s>` text or message; `null` otherwise. */
export function holdErrorSeconds(
  text: string | null | undefined,
): number | null {
  const match = typeof text === "string" ? HOLD_ERROR_PATTERN.exec(text) : null
  return match ? Number(match[1]) : null
}

/**
 * Thrown by the guard instead of sending a call that is under a flood hold.
 * Shaped like an mtcute `RpcError` (`code`, `text`, `seconds`) so existing
 * duck-typed handlers see a 420; `message` and `text` are both the stable
 * `TG_SAI_HOLD_<s>`.
 */
export class HoldError extends Error {
  readonly code = HOLD_ERROR_CODE
  readonly text: string
  readonly seconds: number

  constructor(
    readonly untilMs: number,
    readonly grp: HoldGroup,
    nowMs: number = Date.now(),
    readonly method: string | null = null,
  ) {
    const seconds = remainingSeconds(untilMs, nowMs)
    super(`TG_SAI_HOLD_${seconds}`)
    this.name = "HoldError"
    this.seconds = seconds
    this.text = this.message
  }

  static fromHold(hold: FloodHold, nowMs: number, method: string): HoldError {
    return new HoldError(hold.untilMs, hold.grp, nowMs, method)
  }
}

/** Account the client acts as; `null`/`undefined` disables the guard. */
export type HoldAccountProvider = () => string | number | null | undefined

export interface FloodGuardOptions {
  now?: () => number
  /** Request ID to store with a recorded hold (e.g. telemetry's). */
  requestIdOf?: (ctx: RpcCallMiddlewareContext) => string | null | undefined
}

export interface FloodGuard {
  /** Place between telemetry's outer layer and `basic()`. */
  check: RpcCallMiddleware
  /** Place innermost, after `basic()`: sees only real wire results. */
  record: RpcCallMiddleware
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

function accountOf(provider: HoldAccountProvider): string | null {
  try {
    const id = provider()
    if (id === null || id === undefined) return null
    const text = String(id).trim()
    return text === "" ? null : text
  } catch {
    return null
  }
}

/**
 * Builds the flood-hold middlewares for one client.
 *
 * - `check` throws `HoldError` (code 420, `TG_SAI_HOLD_<s>`) without calling
 *   `next` while the call's group is held. With the store unavailable it
 *   throws `HoldStoreUnavailableError` for invite/create and passes the rest.
 * - `record` saves `FLOOD_WAIT_N`, `FLOOD_PREMIUM_WAIT_N`, and `PEER_FLOOD`
 *   results, returns the result unchanged, and never throws from recording.
 *   Being innermost, it never sees mtcute's synthetic stored-wait results.
 */
export function createFloodGuard(
  store: HoldStore,
  accountProvider: HoldAccountProvider,
  options: FloodGuardOptions = {},
): FloodGuard {
  const now = options.now ?? Date.now

  const check: RpcCallMiddleware = async (ctx, next) => {
    const accountId = accountOf(accountProvider)
    if (accountId === null) return next(ctx)
    const method = String(ctx.request._)
    const grp = methodGroup(method)
    if (store.unavailable) {
      if (isFailClosedGroup(grp)) {
        throw new HoldStoreUnavailableError(
          store.unavailableReason ?? "unavailable",
        )
      }
      return next(ctx)
    }
    const nowMs = now()
    let hold: FloodHold | null
    try {
      hold = store.activeHold(accountId, grp, nowMs)
    } catch (error) {
      if (!isFailClosedGroup(grp)) return next(ctx)
      throw error instanceof HoldStoreUnavailableError
        ? error
        : new HoldStoreUnavailableError(
            error instanceof Error ? error.message : String(error),
          )
    }
    if (hold) throw HoldError.fromHold(hold, nowMs, method)
    return next(ctx)
  }

  const record: RpcCallMiddleware = async (ctx, next) => {
    const result = await next(ctx)
    try {
      if (!isRpcErrorValue(result)) return result
      const parsed = parseHoldError(result.errorMessage)
      if (!parsed) return result
      const accountId = accountOf(accountProvider)
      if (accountId === null || store.unavailable) return result
      const method = String(ctx.request._)
      let requestId: string | null = null
      try {
        requestId = options.requestIdOf?.(ctx) ?? null
      } catch {
        requestId = null
      }
      store.recordHold({
        accountId,
        grp: methodGroup(method),
        method,
        source: parsed.source,
        waitS: parsed.waitS,
        requestId,
        nowMs: now(),
      })
    } catch {
      // Recording must never change the call's result.
    }
    return result
  }

  return { check, record }
}
