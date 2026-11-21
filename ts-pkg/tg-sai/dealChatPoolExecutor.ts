import { randomUUID } from "node:crypto"
import type { ChatInfo, TelegramProvisioner } from "./provisioning"
import {
  definiteCreateRejection,
  PROVISIONING_BOT_ID,
  provisionRetryAt,
  safeProvisionError,
} from "./provisioning"
import {
  hasPoolBotRights,
  isFloodOrHold,
  POOL_BOT_RIGHTS,
  POOL_CHAT_TITLE,
  POOL_RETRY_MS,
  poolChatOwned,
  poolDescription,
  poolHoldEnd,
} from "./poolFiller"
import type { GroupCount, PoolCap, PoolFillResult } from "./poolFiller"
import { spacedAfterMs } from "./state/background"
import type { BackgroundPacing, BackgroundQueue } from "./state/background"
import type { DealChatPoolOutbox } from "./state/dealChatPoolOutbox"
import type { HoldStoreSource } from "./state/guard"
import type { PoolFill, PoolJournal } from "./state/pool"

const STEPS = [
  "create",
  "add_bot",
  "promote_bot",
  "show_history",
  "set_photo",
  "verify",
] as const

type Step = (typeof STEPS)[number]

interface Action {
  accountId: string
  fillId: string
  actionId: string
  leaseToken: string
  leaseExpiresAt: string
  ref: string
  chatId: string | null
  step: Step
  reconcile: boolean
}

interface ResultFields {
  outcome: "succeeded" | "room_failed" | "retry" | "uncertain" | "not_found"
  chatId?: string
  botRights?: Record<string, boolean>
  photoSet?: boolean
  verifiedAt?: string
  errorCode?: string
  retryAt?: string
}

export type DealChatPoolResult = ResultFields &
  Pick<Action, "accountId" | "fillId" | "actionId" | "leaseToken">

export interface DealChatPoolReceipt {
  ref: string
  step: Step
  result: DealChatPoolResult
}

export type LegacyDealChatPoolScan =
  | {
      accountId: string
      ref: string
      legacyStage: string
      legacyStatus: string
      chatId?: string
      botAdds?: number
      errorCode?: string
      retryAt?: string
    }
  | { accountId: string; complete: true }

type ClaimReply = {
  action: Action | null
  cap: number
  groupStopAt: number
  reason?: string
  nextAt?: string
}

export interface DealChatPoolCloud {
  claimDealChatPoolAction(
    input: { accountId: string; workerId: string },
    requestKey: string,
  ): Promise<unknown>
  recordDealChatPoolActionResult(input: DealChatPoolResult): Promise<unknown>
  importDealChatPoolFill(input: LegacyDealChatPoolScan): Promise<unknown>
}

interface ExecutorDependencies {
  accountId: number
  workerId: string
  legacy: Pick<PoolJournal, "openFills" | "nextFillMs">
  outbox: Pick<DealChatPoolOutbox, "get" | "save" | "clear">
  queue: Pick<
    BackgroundQueue,
    "next" | "lastAddMs" | "addsToday" | "recordAdds"
  >
  holds: HoldStoreSource
  pacing: Pick<BackgroundPacing, "gapMs" | "jitter">
  cap: PoolCap
  groups: { value: GroupCount | null }
  cloud: DealChatPoolCloud
  telegram: <T>(
    ids: { fillId: string; actionId: string },
    run: (adapter: TelegramProvisioner) => Promise<T>,
    leaseRemainingMs: number,
  ) => Promise<T>
  photo: () => Promise<Uint8Array>
  now?: () => number
  signal?: AbortSignal
}

export interface DealChatPoolHealth {
  mode: "managed"
  state: "idle" | "working" | "waiting" | "unavailable"
  reason?: string
  step?: string
  nextAt?: string
  lastSuccessAt?: string
}

/** Allowlisted diagnostic identifier. Never log a raw Telegram or API response. */
export function dealChatPoolReason(value: unknown): string | undefined {
  return typeof value === "string" && /^[a-zA-Z][a-zA-Z0-9_]{0,63}$/.test(value)
    ? value
    : undefined
}

function timestamp(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined
  const ms = Date.parse(value)
  return Number.isFinite(ms) ? new Date(ms).toISOString() : undefined
}

function wireId(
  value: unknown,
  sign: "positive" | "negative",
): value is string {
  const pattern = sign === "positive" ? /^[1-9][0-9]*$/ : /^-[1-9][0-9]*$/
  return (
    typeof value === "string" &&
    pattern.test(value) &&
    Number.isSafeInteger(Number(value))
  )
}

function claimFromCloud(raw: unknown, accountId: number): ClaimReply {
  const reply = raw as Record<string, unknown> | null
  if (!reply || typeof reply !== "object")
    throw new Error("Invalid deal chat pool claim")
  const cap = reply.cap
  const groupStopAt = reply.groupStopAt
  if (
    typeof cap !== "number" ||
    !Number.isSafeInteger(cap) ||
    cap < 0 ||
    typeof groupStopAt !== "number" ||
    !Number.isSafeInteger(groupStopAt) ||
    groupStopAt < 1
  )
    throw new Error("Invalid deal chat pool claim limits")
  const rawAction = reply.action
  if (rawAction === null) {
    return {
      action: null,
      cap,
      groupStopAt,
      reason: dealChatPoolReason(reply.reason),
      nextAt: timestamp(reply.nextAt),
    }
  }
  const action = rawAction as Action | undefined
  if (
    !action ||
    typeof action !== "object" ||
    action.accountId !== String(accountId) ||
    !wireId(action.fillId, "positive") ||
    !wireId(action.actionId, "positive") ||
    typeof action.leaseToken !== "string" ||
    !action.leaseToken ||
    !timestamp(action.leaseExpiresAt) ||
    typeof action.ref !== "string" ||
    !/^[0-9a-f]{8}$/.test(action.ref) ||
    !STEPS.includes(action.step) ||
    typeof action.reconcile !== "boolean" ||
    (action.step === "create"
      ? action.chatId !== null
      : !wireId(action.chatId, "negative"))
  )
    throw new Error("Invalid deal chat pool action")
  return { action, cap, groupStopAt }
}

const ROOM_RPC_FAILURES = new Set([
  "CHAT_ID_INVALID",
  "CHANNEL_INVALID",
  "CHANNEL_PRIVATE",
  "CHAT_ADMIN_REQUIRED",
  "CHAT_WRITE_FORBIDDEN",
  "USER_ADMIN_INVALID",
  "ADMINS_TOO_MUCH",
  "BOTS_TOO_MUCH",
  "RIGHT_FORBIDDEN",
])

function importOf(fill: PoolFill, now: number): LegacyDealChatPoolScan {
  const diagnostic = fill.lastError?.split(":").at(-1)
  const errorCode = dealChatPoolReason(diagnostic)
  return {
    accountId: String(fill.accountId),
    ref: fill.ref,
    legacyStage: fill.stage,
    legacyStatus: fill.status,
    ...(fill.chatId === null ? {} : { chatId: String(fill.chatId) }),
    botAdds: fill.botAdds,
    ...(errorCode ? { errorCode } : {}),
    ...(fill.nextAttemptMs > now
      ? { retryAt: new Date(fill.nextAttemptMs).toISOString() }
      : {}),
  }
}

/**
 * Runs Deal Desk's leased action and reports its result. An unacknowledged
 * result is retried verbatim. A process restart leaves recovery to Deal
 * Desk, which only issues an observation after a possibly sent write.
 */
export class DealChatPoolExecutor {
  private imported = false
  private pendingClaim: string | undefined
  private pendingResult: DealChatPoolReceipt | undefined
  private receiptLoaded = false
  private nextAtMs = 0

  private readonly now: () => number

  constructor(private readonly deps: ExecutorDependencies) {
    this.now = deps.now ?? Date.now
  }

  async tick(): Promise<PoolFillResult> {
    const now = this.now()
    if (this.deps.signal?.aborted) return { state: "idle" }
    if (now < this.nextAtMs)
      return { state: "waiting", nextAtMs: this.nextAtMs }
    if (!this.receiptLoaded) {
      try {
        this.pendingResult =
          this.deps.outbox.get(this.deps.accountId) ?? undefined
        this.receiptLoaded = true
      } catch {
        return this.apiUnavailable("worker_receipt_unavailable")
      }
    }
    if (this.pendingResult) return this.flushResult()

    const accountId = String(this.deps.accountId)
    if (!this.imported) {
      try {
        for (const fill of this.deps.legacy.openFills(this.deps.accountId)) {
          const response = await this.deps.cloud.importDealChatPoolFill(
            importOf(fill, now),
          )
          if ((response as { imported?: unknown } | null)?.imported !== true)
            throw new Error("Invalid legacy fill import response")
        }
        const response = await this.deps.cloud.importDealChatPoolFill({
          accountId,
          complete: true,
        })
        if ((response as { managed?: unknown } | null)?.managed !== true)
          throw new Error("Invalid legacy scan completion response")
        this.imported = true
      } catch {
        return this.apiUnavailable("legacy_import_unavailable")
      }
    }

    const requestKey =
      (this.pendingClaim ??= `tg-deal-chat-pool:claim:${randomUUID()}`)
    let rawClaim: unknown
    try {
      rawClaim = await this.deps.cloud.claimDealChatPoolAction(
        { accountId, workerId: this.deps.workerId },
        requestKey,
      )
    } catch {
      return this.apiUnavailable("claim_unavailable")
    }
    this.pendingClaim = undefined
    let claim: ClaimReply
    try {
      claim = claimFromCloud(rawClaim, this.deps.accountId)
    } catch {
      return this.apiUnavailable("invalid_claim")
    }
    this.deps.cap.observe(claim.cap, now)
    const action = claim.action
    if (!action) {
      this.nextAtMs = claim.nextAt
        ? Date.parse(claim.nextAt)
        : now + POOL_RETRY_MS
      return {
        state: "not_allowed",
        reason: claim.reason ?? null,
        nextAtMs: this.nextAtMs,
      }
    }

    const result = await this.perform(action, claim.groupStopAt)
    this.pendingResult = {
      ref: action.ref,
      step: action.step,
      result: {
        accountId: action.accountId,
        fillId: action.fillId,
        actionId: action.actionId,
        leaseToken: action.leaseToken,
        ...result,
      },
    }
    return this.flushResult()
  }

  private apiUnavailable(reason: string): PoolFillResult {
    this.nextAtMs = this.now() + POOL_RETRY_MS
    const pending = this.pendingResult
    return {
      state: "error",
      reason,
      nextAtMs: this.nextAtMs,
      ...(pending
        ? {
            ref: pending.ref,
            stage: pending.step,
            fillId: pending.result.fillId,
            actionId: pending.result.actionId,
          }
        : {}),
    }
  }

  private async flushResult(): Promise<PoolFillResult> {
    const pending = this.pendingResult
    if (!pending) throw new Error("Deal chat pool result is missing")
    const { ref, step, result } = pending
    try {
      this.deps.outbox.save(this.deps.accountId, pending)
    } catch {
      // Keep the result in memory. On restart Rust reconciles the action.
      return this.apiUnavailable("worker_receipt_unavailable")
    }
    try {
      const response =
        await this.deps.cloud.recordDealChatPoolActionResult(result)
      if ((response as { accepted?: unknown } | null)?.accepted !== true)
        throw new Error("Invalid deal chat pool result acknowledgement")
    } catch (error) {
      const code = (error as { refusal?: unknown } | null)?.refusal
      if (
        code === "DEAL_CHAT_POOL_STALE_LEASE" ||
        code === "DEAL_CHAT_POOL_INVALID" ||
        code === "DEAL_CHAT_POOL_IMPORT_REQUIRED"
      ) {
        if (!this.clearResult())
          return this.apiUnavailable("worker_receipt_unavailable")
        if (code === "DEAL_CHAT_POOL_IMPORT_REQUIRED") this.imported = false
      }
      return this.apiUnavailable(
        dealChatPoolReason(code) ?? "action_result_unavailable",
      )
    }
    if (!this.clearResult())
      return this.apiUnavailable("worker_receipt_unavailable")
    const nextAtMs = result.retryAt ? Date.parse(result.retryAt) : undefined
    this.nextAtMs = nextAtMs ?? 0
    const state =
      result.outcome === "succeeded"
        ? step === "verify"
          ? "filled"
          : "progress"
        : result.outcome === "room_failed"
          ? "failed"
          : "waiting"
    return {
      state,
      ref,
      fillId: result.fillId,
      actionId: result.actionId,
      stage: step,
      ...(result.errorCode ? { error: result.errorCode } : {}),
      ...(nextAtMs === undefined ? {} : { nextAtMs }),
    }
  }

  private clearResult(): boolean {
    try {
      this.deps.outbox.clear(this.deps.accountId)
    } catch {
      return false
    }
    this.pendingResult = undefined
    return true
  }

  private checkLease(action: Action) {
    if (
      this.deps.signal?.aborted ||
      this.now() >= Date.parse(action.leaseExpiresAt)
    )
      throw new NoWrite("lease_unavailable")
  }

  private async perform(
    action: Action,
    groupStopAt: number,
  ): Promise<ResultFields> {
    const { accountId, groups, telegram, photo } = this.deps
    let sent = false
    let definitelyRefused = false
    let roomRefused = false
    try {
      this.checkLease(action)
      const ids = { fillId: action.fillId, actionId: action.actionId }
      return await telegram(
        ids,
        async (adapter) => {
          if ((await adapter.self()) !== accountId)
            throw new NoWrite("account_mismatch")

          const send = async <T>(operation: () => Promise<T>): Promise<T> => {
            this.checkLease(action)
            sent = true
            try {
              return await operation()
            } catch (error) {
              const safeCode = safeProvisionError(error)
              definitelyRefused =
                isFloodOrHold(error) ||
                (action.step === "create" && definiteCreateRejection(error))
              roomRefused =
                action.step !== "create" && ROOM_RPC_FAILURES.has(safeCode)
              throw error
            }
          }

          if (action.step === "create") {
            if (action.reconcile) return this.findCreated(action, adapter)
            this.checkAddBudget("create")
            if (!adapter.groupCount)
              throw new NoWrite("group_count_unavailable")
            const count = await adapter.groupCount()
            if (!Number.isSafeInteger(count) || count < 0)
              throw new NoWrite("group_count_unavailable")
            groups.value = { count, atMs: this.now() }
            if (count >= groupStopAt) throw new NoWrite("group_limit")
            this.checkAddBudget("create")
            const created = await send(() =>
              adapter.create(POOL_CHAT_TITLE, poolDescription(action.ref)),
            )
            if (!wireId(String(created), "negative"))
              throw new Error("Invalid created chat identity")
            return { outcome: "succeeded", chatId: String(created) }
          }

          const chatId = Number(action.chatId)
          const info = await this.readRoom(action, adapter, chatId)
          if (!info)
            return { outcome: "room_failed", errorCode: "pool_chat_mismatch" }
          const observeMissing = (): ResultFields =>
            sent ? { outcome: "uncertain" } : { outcome: "not_found" }

          switch (action.step) {
            case "add_bot": {
              if (
                (await adapter.member(chatId, PROVISIONING_BOT_ID)).role !==
                "absent"
              )
                return { outcome: "succeeded" }
              if (action.reconcile) return observeMissing()
              this.checkAddBudget("invite")
              this.checkLease(action)
              // Charge potential adds before a crash can hide the Telegram call.
              // A definite refusal conservatively keeps the charge for this UTC day.
              try {
                this.deps.queue.recordAdds({
                  source: "pool",
                  outcome: "uncertain",
                  chatId,
                  userIds: [PROVISIONING_BOT_ID],
                  nowMs: this.now(),
                })
              } catch {
                throw new NoWrite("add_ledger_unavailable")
              }
              await send(() => adapter.add(chatId, PROVISIONING_BOT_ID))
              return (await adapter.member(chatId, PROVISIONING_BOT_ID))
                .role === "absent"
                ? observeMissing()
                : { outcome: "succeeded" }
            }
            case "promote_bot": {
              if (
                hasPoolBotRights(
                  await adapter.member(chatId, PROVISIONING_BOT_ID),
                )
              )
                return { outcome: "succeeded" }
              if (action.reconcile) return observeMissing()
              if (!adapter.promoteOnce)
                throw new NoWrite("promotion_unavailable")
              await send(() =>
                adapter.promoteOnce!(chatId, PROVISIONING_BOT_ID, {
                  ...POOL_BOT_RIGHTS,
                }),
              )
              return hasPoolBotRights(
                await adapter.member(chatId, PROVISIONING_BOT_ID),
              )
                ? { outcome: "succeeded" }
                : observeMissing()
            }
            case "show_history": {
              if (info.hiddenHistory === false) return { outcome: "succeeded" }
              if (info.hiddenHistory === undefined)
                throw new NoWrite("history_unreadable")
              if (action.reconcile) return observeMissing()
              if (!adapter.showHistory)
                throw new NoWrite("history_toggle_unavailable")
              await send(() => adapter.showHistory!(chatId))
              return (await adapter.chatInfo!(chatId)).hiddenHistory === false
                ? { outcome: "succeeded" }
                : observeMissing()
            }
            case "set_photo": {
              if (info.hasPhoto) return { outcome: "succeeded", photoSet: true }
              if (action.reconcile) return observeMissing()
              if (!adapter.setPhoto)
                return { outcome: "succeeded", photoSet: false }
              let jpeg: Uint8Array
              try {
                jpeg = await photo()
              } catch {
                return { outcome: "succeeded", photoSet: false }
              }
              try {
                await send(() => adapter.setPhoto!(chatId, jpeg))
              } catch (error) {
                // A confirmed room-level refusal can skip the cosmetic step.
                if (roomRefused)
                  return { outcome: "succeeded", photoSet: false }
                throw error
              }
              return (await adapter.chatInfo!(chatId)).hasPhoto === true
                ? { outcome: "succeeded", photoSet: true }
                : observeMissing()
            }
            case "verify": {
              const bot = await adapter.member(chatId, PROVISIONING_BOT_ID)
              if (!hasPoolBotRights(bot) || info.hiddenHistory !== false)
                return {
                  outcome: "room_failed",
                  errorCode: "room_verification_failed",
                }
              return {
                outcome: "succeeded",
                botRights: Object.fromEntries(
                  Object.entries(POOL_BOT_RIGHTS).filter(
                    ([key]) => bot.rights[key] === true,
                  ),
                ),
                photoSet: info.hasPhoto === true,
                verifiedAt: new Date(this.now()).toISOString(),
              }
            }
          }
        },
        Math.max(1, Date.parse(action.leaseExpiresAt) - this.now()),
      )
    } catch (error) {
      const errorCode =
        error instanceof NoWrite ? error.reason : safeProvisionError(error)
      if (
        roomRefused ||
        (action.step !== "create" &&
          !sent &&
          (errorCode === "CHANNEL_INVALID" || errorCode === "CHAT_ID_INVALID"))
      )
        return { outcome: "room_failed", errorCode }
      const retryAt =
        error instanceof NoWrite && error.retryAt
          ? new Date(error.retryAt).toISOString()
          : (provisionRetryAt(error, this.now()) ??
            new Date(this.now() + POOL_RETRY_MS).toISOString())
      const outcome = sent && !definitelyRefused ? "uncertain" : "retry"
      return { outcome, errorCode, retryAt }
    }
  }

  private async readRoom(
    action: Action,
    adapter: TelegramProvisioner,
    chatId: number,
  ): Promise<ChatInfo | null> {
    if (!adapter.chatInfo) throw new NoWrite("chat_info_unavailable")
    const info = await adapter.chatInfo(chatId)
    // The issuing account must be the owner, even if other accounts are configured.
    return poolChatOwned(
      info,
      action.ref,
      [this.deps.accountId],
      this.deps.accountId,
      null,
    )
      ? info
      : null
  }

  private async findCreated(
    action: Action,
    adapter: TelegramProvisioner,
  ): Promise<ResultFields> {
    if (!adapter.poolCandidates)
      throw new NoWrite("candidate_search_unavailable")
    const ids = await adapter.poolCandidates(action.ref)
    if (ids.length > 1) throw new NoWrite("duplicate_pool_reference")
    if (!ids.length) return { outcome: "not_found" }
    const chatId = ids[0]!
    if (
      !wireId(String(chatId), "negative") ||
      !(await this.readRoom(action, adapter, chatId))
    )
      throw new NoWrite("pool_candidate_mismatch")
    return { outcome: "succeeded", chatId: String(chatId) }
  }

  private checkAddBudget(group: "create" | "invite") {
    const now = this.now()
    const { queue, holds, pacing, cap, accountId, legacy } = this.deps
    if (queue.next(now)) throw new NoWrite("waiting_on_background")
    const hold = poolHoldEnd(
      holds,
      accountId,
      group === "create" ? ["create", "invite"] : ["invite"],
      now,
    )
    if (hold !== null) throw new NoWrite("hold", hold)
    const paced = Math.max(
      spacedAfterMs(queue.lastAddMs(), pacing) ?? 0,
      legacy.nextFillMs(accountId),
    )
    if (paced > now) throw new NoWrite("paced", paced)
    if (queue.addsToday(now) >= cap.current(now))
      throw new NoWrite(
        "daily_cap",
        (Math.floor(now / 86_400_000) + 1) * 86_400_000,
      )
  }
}

class NoWrite extends Error {
  constructor(
    readonly reason: string,
    readonly retryAt?: number,
  ) {
    super(reason)
  }
}
