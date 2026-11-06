import type { RpcCallMiddleware, RpcCallMiddlewareContext } from "@mtcute/bun"

import {
  HoldStore,
  HoldStoreUnavailableError,
  isFailClosedGroup,
  methodGroup,
  parseHoldError,
  remainingSeconds,
  type FloodHold,
  type HoldGroup,
  type ParsedHold,
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
  /**
   * The process flood waiter's `maxWait`. A wait at or under it (or under
   * the call's own `floodSleepThreshold`) is slept through and retried by
   * mtcute, so it is not saved, except on invite/create and `PEER_FLOOD`.
   * Unset: every flood wait is saved.
   */
  sleepThresholdMs?: number
  /** Request ID to store with a recorded hold (e.g. telemetry's). */
  requestIdOf?: (ctx: RpcCallMiddlewareContext) => string | null | undefined
}

/** A store, or a provider called on each use (e.g. a lazy singleton). */
export type HoldStoreSource = HoldStore | (() => HoldStore)

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

/**
 * Whether `record` saves a parsed hold. Invite/create and `PEER_FLOOD` are
 * always saved; other waits only when mtcute will not sleep through them.
 */
export function shouldSaveHold(
  grp: HoldGroup,
  parsed: ParsedHold,
  thresholdMs: number | undefined,
): boolean {
  if (isFailClosedGroup(grp) || parsed.source === "peer_flood") return true
  if (thresholdMs === undefined) return true
  return parsed.waitS * 1000 > thresholdMs
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
 *   results that `shouldSaveHold` accepts, returns the result unchanged, and
 *   never throws from recording.
 *   Being innermost, it never sees mtcute's synthetic stored-wait results.
 */
export function createFloodGuard(
  source: HoldStoreSource,
  accountProvider: HoldAccountProvider,
  options: FloodGuardOptions = {},
): FloodGuard {
  const now = options.now ?? Date.now
  const storeOf = (): HoldStore => {
    if (typeof source !== "function") return source
    try {
      return source()
    } catch (error) {
      return HoldStore.unavailable(
        error instanceof Error ? error.message : String(error),
      )
    }
  }

  const check: RpcCallMiddleware = async (ctx, next) => {
    const accountId = accountOf(accountProvider)
    if (accountId === null) return next(ctx)
    const method = String(ctx.request._)
    const grp = methodGroup(method)
    const store = storeOf()
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
      if (accountId === null) return result
      const method = String(ctx.request._)
      const grp = methodGroup(method)
      const threshold =
        ctx.params?.floodSleepThreshold ?? options.sleepThresholdMs
      if (!shouldSaveHold(grp, parsed, threshold)) return result
      const store = storeOf()
      if (store.unavailable) return result
      let requestId: string | null = null
      try {
        requestId = options.requestIdOf?.(ctx) ?? null
      } catch {
        requestId = null
      }
      store.recordHold({
        accountId,
        grp,
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
