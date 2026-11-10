import {
  isPerUserAddError,
  isPrivacyRestriction,
  isTransientTelegramError,
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

/**
 * `adapter` with `add` and `addMany` logging each user Telegram added, or
 * refused for that user alone, to `queue` as `setup` adds. Privacy
 * refusals and whole-call errors (floods, holds) log nothing. A failed
 * log write never fails the add.
 */
export function recordingMemberAdds(
  adapter: TelegramProvisioner,
  queue: Pick<BackgroundQueue, "recordAdds">,
  now: () => number = Date.now,
): TelegramProvisioner {
  const record = (
    chatId: number,
    userIds: number[],
    outcome: "added" | "refused",
  ) => {
    if (!userIds.length) return
    try {
      queue.recordAdds({
        source: "setup",
        outcome,
        chatId,
        userIds,
        nowMs: now(),
      })
    } catch {
      // The add happened; a missed count only undercounts today.
    }
  }
  const addMany = adapter.addMany?.bind(adapter)
  return {
    ...adapter,
    add: async (chatId, userId) => {
      try {
        await adapter.add(chatId, userId)
      } catch (error) {
        if (isPerUserAddError(error) && !isPrivacyRestriction(error))
          record(chatId, [userId], "refused")
        throw error
      }
      record(chatId, [userId], "added")
    },
    ...(addMany
      ? {
          // A refused batch falls back to `add` per user, which logs.
          addMany: async (chatId: number, userIds: number[]) => {
            const missing = await addMany(chatId, userIds)
            const skipped = new Set(missing)
            record(
              chatId,
              userIds.filter((id) => !skipped.has(id)),
              "added",
            )
            return missing
          },
        }
      : {}),
  }
}

/** What one drain step did. */
export interface DrainResult {
  state:
    | "idle"
    | "capped"
    | "held"
    | "added"
    | "joined"
    | "privacy"
    | "failed"
    | "deferred"
    | "error"
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
 * hold is active. A member who already joined, usually through the shared
 * link, is marked `added` without an add call (`joined`), so the account
 * spends no add on them and today's count is unchanged. The same holds for
 * a member whose join request through that link is waiting: the drain
 * approves it. A flood or hold on the read or approval defers the row until
 * it ends, uncounted; a chat or member fault (`isMemberFault`) fails the
 * row; any other error there counts an attempt. The cap counts every member add the account made today
 * (`BackgroundQueue.addsToday`, setup adds included), so background adds
 * stop once the total reaches `dailyMax`. Success marks the row `added`; a privacy refusal marks it
 * `privacy` and makes no link, since the ready receipt's shared
 * `background_queue` link already covers the member, and does not count
 * toward the daily cap; another per-user refusal is `failed`. A flood or
 * hold leaves the row pending until it ends. A transient error
 * (`isTransientTelegramError`) from the read, approval, or add defers the
 * row behind the others without counting an attempt. Other errors from
 * the add call retry after a gap up to `MAX_BACKGROUND_ATTEMPTS`. Errors before or
 * around the call (session, connect, identity checks, shutdown) defer the
 * row one gap without counting an attempt. Every call paces the remaining
 * rows by one jittered gap. Does nothing once `signal` is aborted. Throws
 * only when the queue itself fails.
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
  /** Worker shutdown: no drain starts, and a cut-off call is not counted. */
  signal?: AbortSignal
}): Promise<DrainResult> {
  const { queue, pacing } = params
  if (params.signal?.aborted) return { state: "idle" }
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
  // Set only by an error the add call itself threw.
  let callError: { error: unknown } | null = null
  // Set only by an error from the member read or the approval.
  let memberError: { error: unknown } | null = null
  const outcome = await params
    .telegram(async (telegram) => {
      try {
        const member = await telegram.member(row.chatId, row.userId)
        if (member.role !== "absent") return { state: "joined" as const }
        // The shared link needs approval; a pending request is approved
        // instead of spending an add. An error here stops before the add.
        if (await telegram.approveJoin?.(row.chatId, row.userId))
          return { state: "joined" as const }
      } catch (error) {
        memberError = { error }
        throw error
      }
      try {
        await telegram.add(row.chatId, row.userId)
        return { state: "added" as const }
      } catch (error) {
        if (isPrivacyRestriction(error))
          return {
            state: "privacy" as const,
            error: safeProvisionError(error),
          }
        callError = { error }
        throw error
      }
    })
    .catch((error: unknown) => ({ state: "error" as const, error }))
  if (outcome.state !== "error") {
    if (outcome.state === "added")
      queue.recordAdds({
        source: "background",
        outcome: "added",
        chatId: row.chatId,
        userIds: [row.userId],
        nowMs,
      })
    queue.finish(
      row.chatId,
      row.userId,
      outcome.state === "joined" ? "added" : outcome.state,
      outcome.state === "privacy" ? outcome.error : null,
      nowMs,
    )
    queue.pace(paced)
    return {
      state: outcome.state,
      ...at,
      ...(outcome.state === "privacy" ? { error: outcome.error } : {}),
      nextAtMs: paced,
    }
  }
  const code = safeProvisionError(outcome.error)
  const fromCall =
    (callError as { error: unknown } | null)?.error === outcome.error
  const fromMember =
    (memberError as { error: unknown } | null)?.error === outcome.error
  // A counted retry goes behind the rows paced to `paced`, so one stuck row
  // never starves the rest of the queue.
  const behind = paced + pacing.gapMs
  if (
    (fromMember || fromCall) &&
    !params.signal?.aborted &&
    isTransientTelegramError(outcome.error)
  ) {
    // A timeout or dropped connection is no one's fault: uncounted, like a
    // flood, but behind the other rows like a counted retry. A lost add
    // response is safe because the next step reads the member first.
    queue.defer(row.chatId, row.userId, behind, code, nowMs, false)
    queue.pace(paced)
    return { state: "deferred", ...at, error: code, nextAtMs: paced }
  }
  if (fromMember && !params.signal?.aborted) {
    const retryAt = provisionRetryAt(outcome.error, nowMs)
    if (retryAt) {
      const until = Math.max(Date.parse(retryAt), paced)
      queue.defer(row.chatId, row.userId, until, code, nowMs, false)
      queue.pace(until)
      return { state: "deferred", ...at, error: code, nextAtMs: until }
    }
    // This chat or member cannot be read or approved: no add can help.
    if (
      isMemberFault(outcome.error) ||
      row.attempts + 1 >= MAX_BACKGROUND_ATTEMPTS
    ) {
      queue.finish(row.chatId, row.userId, "failed", code, nowMs)
      queue.pace(paced)
      return { state: "failed", ...at, error: code, nextAtMs: paced }
    }
    queue.defer(row.chatId, row.userId, behind, code, nowMs)
    queue.pace(paced)
    return { state: "deferred", ...at, error: code, nextAtMs: paced }
  }
  if (!fromCall || params.signal?.aborted) {
    // The member was never at fault: retry after one gap, uncounted.
    queue.defer(row.chatId, row.userId, paced, code, nowMs, false)
    queue.pace(paced)
    return { state: "deferred", ...at, error: code, nextAtMs: paced }
  }
  const retryAt = provisionRetryAt(outcome.error, nowMs)
  if (retryAt) {
    const until = Math.max(Date.parse(retryAt), paced)
    // A flood or hold is the account's limit, not the member's failure.
    queue.defer(row.chatId, row.userId, until, code, nowMs, false)
    queue.pace(until)
    return { state: "deferred", ...at, error: code, nextAtMs: until }
  }
  if (
    isPerUserAddError(outcome.error) ||
    row.attempts + 1 >= MAX_BACKGROUND_ATTEMPTS
  ) {
    // Only a per-user refusal is known to have reached Telegram.
    if (isPerUserAddError(outcome.error))
      queue.recordAdds({
        source: "background",
        outcome: "refused",
        chatId: row.chatId,
        userIds: [row.userId],
        nowMs,
      })
    queue.finish(row.chatId, row.userId, "failed", code, nowMs)
    queue.pace(paced)
    return { state: "failed", ...at, error: code, nextAtMs: paced }
  }
  queue.defer(row.chatId, row.userId, behind, code, nowMs)
  queue.pace(paced)
  return { state: "deferred", ...at, error: code, nextAtMs: paced }
}

/**
 * A read or approval error that belongs to this chat or member: per-user
 * refusals, an inaccessible chat or peer, or a deleted account.
 */
export function isMemberFault(error: unknown): boolean {
  if (isPerUserAddError(error)) return true
  const e = error as { text?: unknown; errorMessage?: unknown }
  const text = typeof e?.text === "string" ? e.text : e?.errorMessage
  return (
    typeof text === "string" &&
    /^(?:CHANNEL_PRIVATE|CHANNEL_INVALID|CHAT_ID_INVALID|PEER_ID_INVALID|USER_DEACTIVATED|INPUT_USER_DEACTIVATED|USER_CHANNELS_TOO_MUCH|CHANNELS_TOO_MUCH)$/.test(
      text,
    )
  )
}
