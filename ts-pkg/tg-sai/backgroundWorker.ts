import {
  isPerUserAddError,
  isPrivacyRestriction,
  provisionRetryAt,
  safeProvisionError,
} from "./provisioning"
import type { BackgroundEnqueue, TelegramProvisioner } from "./provisioning"
import { openStateDb, stateDbPath } from "./state/db"
import type { HoldStoreSource } from "./state/guard"
import {
  BackgroundQueue,
  backgroundPacing,
  jitteredGapMs,
  MAX_BACKGROUND_ATTEMPTS,
  nextUtcDayMs,
} from "./state/background"
import type { BackgroundPacing } from "./state/background"
import type { ProfileStoreEnv } from "./profiles"

/** Opens the queue in `state.db` under `env`'s config dir. Throws on failure. */
export function openBackgroundQueue(
  env: ProfileStoreEnv = process.env,
): BackgroundQueue {
  return new BackgroundQueue(openStateDb(stateDbPath(env)))
}

/**
 * `BackgroundEnqueue` over `queue`: new rows wait one jittered gap, so the
 * first background add never follows a setup's own adds at once.
 */
export function backgroundEnqueue(
  queue: BackgroundQueue,
  pacing: BackgroundPacing = backgroundPacing(),
  options: { now?: () => number; random?: () => number } = {},
): BackgroundEnqueue {
  const now = options.now ?? Date.now
  return {
    enqueue: (input) => {
      const nowMs = now()
      queue.enqueue({
        ...input,
        notBeforeMs: nowMs + jitteredGapMs(pacing, options.random),
        nowMs,
      })
    },
  }
}

/** What one drain step did. */
export interface DrainResult {
  state:
    | "idle"
    | "capped"
    | "held"
    | "added"
    | "invited"
    | "failed"
    | "deferred"
  chatId?: number
  userId?: number
  error?: string
  /** When the next add may run, if known. */
  nextAtMs?: number
}

/** Latest invite-hold end for `accountId`; unreadable store = 5 min hold. */
function inviteHoldUntil(
  holds: HoldStoreSource,
  accountId: number,
  nowMs: number,
): number | null {
  try {
    const store = typeof holds === "function" ? holds() : holds
    const hold = store.activeHold(accountId, "invite", nowMs)
    return hold && hold.untilMs > nowMs ? hold.untilMs : null
  } catch {
    return nowMs + 5 * 60 * 1000
  }
}

/**
 * One background add, when one is due, the daily cap allows, and no invite
 * hold is active. Success marks the row `added`; a privacy refusal gets an
 * invite link and `invited`; a per-user refusal is `failed`. A flood or
 * hold leaves the row pending until it ends, and other errors retry after a
 * gap up to `MAX_BACKGROUND_ATTEMPTS`. Every call paces the remaining rows
 * by one jittered gap. Throws only when the queue itself fails.
 */
export async function drainBackgroundOnce(params: {
  queue: BackgroundQueue
  holds: HoldStoreSource
  accountId: number
  pacing: BackgroundPacing
  telegram: <T>(
    operation: (adapter: TelegramProvisioner) => Promise<T>,
  ) => Promise<T>
  nowMs?: number
  random?: () => number
}): Promise<DrainResult> {
  const { queue, pacing } = params
  const nowMs = params.nowMs ?? Date.now()
  const row = queue.next(nowMs)
  if (!row) return { state: "idle" }
  const at = { chatId: row.chatId, userId: row.userId }
  if (queue.addsToday(nowMs) >= pacing.dailyMax) {
    const nextAtMs = nextUtcDayMs(nowMs)
    queue.pace(nextAtMs)
    return { state: "capped", ...at, nextAtMs }
  }
  const held = inviteHoldUntil(params.holds, params.accountId, nowMs)
  if (held !== null) {
    queue.pace(held)
    return { state: "held", ...at, nextAtMs: held }
  }
  const paced = nowMs + jitteredGapMs(pacing, params.random)
  const outcome = await params
    .telegram(async (telegram) => {
      try {
        await telegram.add(row.chatId, row.userId)
        return { state: "added" as const }
      } catch (error) {
        if (!isPrivacyRestriction(error) || !telegram.inviteLink) throw error
        await telegram.inviteLink(row.chatId, row.userId)
        return { state: "invited" as const, error: safeProvisionError(error) }
      }
    })
    .catch((error: unknown) => ({ state: "error" as const, error }))
  if (outcome.state !== "error") {
    queue.finish(
      row.chatId,
      row.userId,
      outcome.state,
      outcome.state === "invited" ? outcome.error : null,
      nowMs,
    )
    queue.pace(paced)
    return {
      state: outcome.state,
      ...at,
      ...(outcome.state === "invited" ? { error: outcome.error } : {}),
      nextAtMs: paced,
    }
  }
  const code = safeProvisionError(outcome.error)
  const retryAt = provisionRetryAt(outcome.error, nowMs)
  if (retryAt) {
    const until = Math.max(Date.parse(retryAt), paced)
    queue.defer(row.chatId, row.userId, until, code, nowMs)
    queue.pace(until)
    return { state: "deferred", ...at, error: code, nextAtMs: until }
  }
  if (
    isPerUserAddError(outcome.error) ||
    row.attempts + 1 >= MAX_BACKGROUND_ATTEMPTS
  ) {
    queue.finish(row.chatId, row.userId, "failed", code, nowMs)
    queue.pace(paced)
    return { state: "failed", ...at, error: code, nextAtMs: paced }
  }
  queue.defer(row.chatId, row.userId, paced, code, nowMs)
  queue.pace(paced)
  return { state: "deferred", ...at, error: code, nextAtMs: paced }
}
