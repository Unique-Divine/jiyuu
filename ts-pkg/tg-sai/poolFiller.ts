import { randomBytes } from "node:crypto"
import {
  CHAT_DESCRIPTION_TEXT,
  definiteCreateRejection,
  descriptionRef,
  isPerUserAddError,
  isTransientTelegramError,
  MEMBER_RECHECK_MS,
  provisionRetryAt,
  PROVISIONING_BOT_ID,
  safeProvisionError,
} from "./provisioning"
import type { ChatInfo, Member, TelegramProvisioner } from "./provisioning"
import type { ProfileStoreEnv } from "./profiles"
import type { BackgroundQueue, BackgroundPacing } from "./state/background"
import { jitteredGapMs } from "./state/background"
import type { HoldStoreSource } from "./state/guard"
import type { HoldGroup } from "./state/holds"
import type { PoolFill, PoolJournal, PoolUpdate } from "./state/pool"

/** Title of a pool chat until Deal Desk claims and renames it. */
export const POOL_CHAT_TITLE = "Sai deal chat (available)"

/**
 * The bot's rights in a pool chat: every admin right a supergroup has,
 * short of owner. Channel-only rights (`postMessages`, `editMessages`,
 * `manageDirectMessages`) stay off, and so does `anonymous`, so the bot's
 * messages show as the bot. Telegram lets an admin grant only rights it
 * holds, so these are also what lets the bot promote the team.
 */
export const POOL_BOT_RIGHTS: Readonly<Record<string, boolean>> = {
  changeInfo: true,
  postMessages: false,
  editMessages: false,
  deleteMessages: true,
  banUsers: true,
  inviteUsers: true,
  pinMessages: true,
  addAdmins: true,
  anonymous: false,
  manageCall: true,
  other: true,
  manageTopics: true,
  postStories: true,
  editStories: true,
  deleteStories: true,
  manageDirectMessages: false,
  manageRanks: true,
}

/**
 * Rights the bot must show after promotion: Deal Desk's `available`
 * requirement plus `manageCall` and `other`. Topic and story rights are
 * granted but not required, since Telegram may drop them on a non-forum
 * supergroup.
 */
export const POOL_BOT_REQUIRED_RIGHTS: readonly string[] = [
  "changeInfo",
  "deleteMessages",
  "banUsers",
  "inviteUsers",
  "pinMessages",
  "addAdmins",
  "manageCall",
  "other",
]

/** Hold groups a fill uses: create the chat and add the bot. */
export const POOL_HOLD_GROUPS: readonly HoldGroup[] = ["create", "invite"]

/** A transient failure is retried this many times, then the fill fails. */
export const MAX_POOL_ATTEMPTS = 5
/** Delay before retrying a transient failure. */
export const POOL_RETRY_MS = 5 * 60 * 1000
/** Delay before re-reading a bot that was added but is not shown yet. */
export const POOL_BOT_RECHECK_MS = 15 * 60 * 1000
/** Reads of an added bot that may miss it before the fill fails. */
export const MAX_BOT_VISIBILITY_CHECKS = 4
/**
 * Dialog searches for a chat whose create timed out. After this many
 * searches 5 minutes apart find nothing, the create did not happen.
 */
export const MAX_CREATION_SEARCHES = 3
/** Wait before asking Deal Desk again after it said not to fill. */
export const POOL_STATUS_RECHECK_MS = 5 * 60 * 1000
/** A cap read from Deal Desk is used for this long. */
export const POOL_CAP_TTL_MS = 60 * 60 * 1000
/** The account's group count is read at most this often. */
export const GROUP_COUNT_INTERVAL_MS = 60 * 60 * 1000

export const OWNER_ACCOUNTS_ENV = "TG_SAI_OWNER_ACCOUNTS"

/**
 * Accounts allowed to own pool chats, from `TG_SAI_OWNER_ACCOUNTS` (comma-
 * or space-separated numeric user IDs). Unset or blank means `fallback`
 * alone (the custody account). Throws on an invalid entry, so a typo never
 * widens or empties the list.
 */
export function ownerAccounts(
  fallback: number,
  env: ProfileStoreEnv = process.env,
): number[] {
  const raw = env[OWNER_ACCOUNTS_ENV]
  if (raw === undefined || !raw.trim()) return [fallback]
  const ids = raw
    .split(/[\s,]+/)
    .filter(Boolean)
    .map((entry) => {
      if (!/^[1-9][0-9]{0,15}$/.test(entry))
        throw new Error(`Invalid ${OWNER_ACCOUNTS_ENV} entry`)
      const id = Number(entry)
      if (!Number.isSafeInteger(id) || id === PROVISIONING_BOT_ID)
        throw new Error(`Invalid ${OWNER_ACCOUNTS_ENV} entry`)
      return id
    })
  return [...new Set(ids)]
}

/** A fresh pool ref: 8 lowercase hex characters. */
export function newPoolRef(bytes: () => Uint8Array = () => randomBytes(4)): string {
  const ref = Buffer.from(bytes()).toString("hex").slice(0, 8)
  if (!/^[0-9a-f]{8}$/.test(ref)) throw new Error("Invalid pool ref")
  return ref
}

/** Description of a pool chat; the ref never changes on claim. */
export function poolDescription(ref: string): string {
  if (!/^[0-9a-f]{8}$/.test(ref)) throw new Error("Invalid pool ref")
  return `${CHAT_DESCRIPTION_TEXT} Ref: ${ref}`
}

/**
 * Whether a chat is pool chat `ref` owned by an allowed account: a
 * non-forum supergroup whose description ends in `Ref: <ref>`, created
 * either by the reading account (`selfId`) or by `creatorId`, and that
 * creator is in `owners`.
 */
export function poolChatOwned(
  chat: Pick<ChatInfo, "chatType" | "isForum" | "isCreator" | "description">,
  ref: string,
  owners: readonly number[],
  selfId: number,
  creatorId: number | null = null,
): boolean {
  if (chat.chatType !== "supergroup" || chat.isForum) return false
  if (descriptionRef(chat.description) !== ref) return false
  const creator = chat.isCreator ? selfId : creatorId
  return creator !== null && owners.includes(creator)
}

/** Whether `member` is an admin with every `POOL_BOT_REQUIRED_RIGHTS`. */
export function hasPoolBotRights(member: Member): boolean {
  return (
    member.role === "admin" &&
    POOL_BOT_REQUIRED_RIGHTS.every((key) => member.rights[key] === true)
  )
}

/** `pool-status` reply fields the filler reads (Deal Desk contract). */
export interface PoolStatusReply {
  accountId: string
  allowedNow: boolean
  reason: string | null
  /** The account's daily add cap for today. */
  cap: number | null
  remaining?: number | null
  needed?: number
}

/** `pool-register` input. */
export interface PoolRegistration {
  accountId: string
  chatId: string
  state: "filling" | "available"
  ref: string
  title: string
  botUserId: string
  botRights?: Record<string, boolean>
  photoSet: boolean
  verifiedAt?: string
}

/** `stage` values `pool-fail` takes. */
export type PoolFailStage =
  | "create"
  | "add_bot"
  | "promote_bot"
  | "photo"
  | "description"
  | "history"
  | "register"
  | "other"

/** `pool-fail` input. */
export interface PoolFailure {
  accountId: string
  chatId?: string
  stage: PoolFailStage
  error: string
  floodWaitS: number | null
}

/** Deal Desk's pool operations. */
export interface PoolCloud {
  poolStatus(accountId: number): Promise<PoolStatusReply>
  poolRegister(input: PoolRegistration): Promise<unknown>
  poolFail(input: PoolFailure): Promise<unknown>
}

/**
 * The account's daily add cap: Deal Desk's value from the last
 * `pool-status` read within `POOL_CAP_TTL_MS`, else `fallback` (the fixed
 * `TG_SAI_BACKGROUND_DAILY_MAX`). The background drain and the filler
 * both read it.
 */
export class PoolCap {
  private value: number | null = null
  private atMs = -Infinity
  constructor(private readonly fallback: number) {}

  /** Saves a cap Deal Desk returned; ignores anything but a whole number ≥ 0. */
  observe(cap: unknown, nowMs: number): void {
    if (typeof cap === "number" && Number.isSafeInteger(cap) && cap >= 0) {
      this.value = cap
      this.atMs = nowMs
    }
  }

  /** The cap to use at `nowMs`. */
  current(nowMs: number): number {
    return this.value !== null && nowMs - this.atMs < POOL_CAP_TTL_MS
      ? this.value
      : this.fallback
  }

  /** Where the current cap came from. */
  source(nowMs: number): "deal_desk" | "fixed" {
    return this.value !== null && nowMs - this.atMs < POOL_CAP_TTL_MS
      ? "deal_desk"
      : "fixed"
  }
}

/** Last group count read in a fill session. */
export interface GroupCount {
  count: number
  atMs: number
}

/** What one fill step did. */
export interface PoolFillResult {
  state:
    | "idle"
    | "paced"
    | "held"
    | "not_allowed"
    | "capped"
    | "waiting"
    | "blocked"
    | "progress"
    | "filled"
    | "retired"
    | "failed"
    | "error"
  ref?: string
  chatId?: number
  stage?: string
  /** Deal Desk's `reason` when it did not allow a fill. */
  reason?: string | null
  error?: string
  nextAtMs?: number
}

/** Fill states that made no Telegram call. */
export const IDLE_POOL_STATES: ReadonlySet<string> = new Set([
  "idle",
  "paced",
  "held",
  "not_allowed",
  "capped",
  "waiting",
  "blocked",
])

/** Latest hold end on `groups`; an unreadable store holds 5 minutes. */
function holdEnd(
  holds: HoldStoreSource,
  accountId: number,
  groups: readonly HoldGroup[],
  nowMs: number,
): number | null {
  try {
    const store = typeof holds === "function" ? holds() : holds
    let until = 0
    for (const grp of groups) {
      const hold = store.activeHold(accountId, grp, nowMs)
      if (hold && hold.untilMs > until) until = hold.untilMs
    }
    return until > nowMs ? until : null
  } catch {
    return nowMs + 5 * 60 * 1000
  }
}

/** A flood wait, `PEER_FLOOD`, or a local hold refusal. */
function isFloodOrHold(error: unknown): boolean {
  const e = error as { text?: unknown; errorMessage?: unknown }
  const text = typeof e?.text === "string" ? e.text : e?.errorMessage
  return (
    typeof text === "string" &&
    /^(?:FLOOD(?:_PREMIUM)?_WAIT_|PEER_FLOOD$|TG_SAI_HOLD_)/.test(text)
  )
}

/** Telegram's wait in seconds for a `FLOOD_WAIT_N`, else `null`. */
function floodWaitSeconds(error: unknown): number | null {
  const e = error as { text?: unknown; seconds?: unknown }
  const match =
    typeof e?.text === "string"
      ? /^FLOOD(?:_PREMIUM)?_WAIT_(\d+|%d)$/.exec(e.text)
      : null
  if (!match) return null
  const seconds = match[1] === "%d" ? e.seconds : Number(match[1])
  return typeof seconds === "number" && Number.isSafeInteger(seconds)
    ? seconds
    : null
}

/** Internal step names, saved in `last_error` as `<step>:<code>`. */
type FillStep =
  | "create"
  | "find_created"
  | "verify_chat"
  | "add_bot"
  | "read_bot"
  | "promote_bot"
  | "photo"
  | "register"

const FAIL_STAGE: Record<FillStep, PoolFailStage> = {
  create: "create",
  find_created: "create",
  verify_chat: "other",
  add_bot: "add_bot",
  read_bot: "add_bot",
  promote_bot: "promote_bot",
  photo: "photo",
  register: "register",
}

/** A stop that ends the step loop with `result`. */
class Stop {
  constructor(readonly result: PoolFillResult) {}
}

/** Error with a fixed code, for checks the filler makes itself. */
function coded(text: string, message: string): Error {
  return Object.assign(new Error(message), { text })
}

/**
 * One step of the deal chat pool filler for `accountId` (boku#77). At most
 * one pool chat is in progress per account, and each call works on it or
 * starts the next one:
 *
 * 1. A `failed` fill blocks every new fill until an operator retires it.
 *    An `active` fill resumes at its stage once its retry time is due.
 * 2. A new fill needs its pacing gap passed (`pool_pace`), no saved hold on
 *    `create` or `invite`, Deal Desk's `pool-status` `allowedNow`, and a
 *    member add left under today's cap (`PoolCap`, local `addsToday`).
 * 3. Create the supergroup `POOL_CHAT_TITLE` with `poolDescription(ref)`,
 *    register it `filling`, add the bot (exactly one add, journaled before
 *    the call and counted in `member_add` as source `pool`, uncertain
 *    outcomes included), promote it to `POOL_BOT_RIGHTS`, set the Sai
 *    photo, then register it `available`. History stays visible.
 *
 * A flood or hold stops the fill until it ends and paces the next one past
 * it. A transient error retries after `POOL_RETRY_MS`, at most
 * `MAX_POOL_ATTEMPTS` times. Anything else fails the fill: the record keeps
 * its chat, Deal Desk gets `pool-fail`, and no new fill starts until an
 * operator acts. A create whose result is unknown is searched for by its
 * ref, never created again.
 */
export async function fillPoolOnce(params: {
  accountId: number
  owners: readonly number[]
  journal: PoolJournal
  queue: Pick<BackgroundQueue, "addsToday" | "recordAdds">
  holds: HoldStoreSource
  pacing: Pick<BackgroundPacing, "gapMs" | "jitter">
  cap: PoolCap
  cloud: PoolCloud
  telegram: <T>(
    operation: (adapter: TelegramProvisioner) => Promise<T>,
  ) => Promise<T>
  /** JPEG bytes of the chat photo. */
  photo: () => Promise<Uint8Array>
  /** Last group count; refreshed in a fill session when stale. */
  groups?: { value: GroupCount | null }
  now?: () => number
  random?: () => number
  newRef?: () => string
  sleep?: (ms: number) => Promise<void>
  signal?: AbortSignal
}): Promise<PoolFillResult> {
  const { accountId, journal } = params
  const now = params.now ?? Date.now
  if (params.signal?.aborted) return { state: "idle" }
  if (!params.owners.includes(accountId))
    throw new Error("Account is not a configured pool owner account")
  const startMs = now()
  let fill = journal.open(accountId)
  const at = (f: PoolFill) => ({
    ref: f.ref,
    ...(f.chatId !== null ? { chatId: f.chatId } : {}),
    stage: f.stage,
  })
  if (fill?.status === "failed")
    return {
      state: "blocked",
      ...at(fill),
      ...(fill.lastError ? { error: fill.lastError } : {}),
    }
  if (fill && fill.nextAttemptMs > startMs)
    return { state: "waiting", ...at(fill), nextAtMs: fill.nextAttemptMs }
  const capLeft = (nowMs: number) =>
    params.queue.addsToday(nowMs) < params.cap.current(nowMs)
  const nextUtcDay = (nowMs: number) =>
    Math.floor(nowMs / 86_400_000) * 86_400_000 + 86_400_000
  let fresh = false
  let createSent = false
  if (!fill) {
    const paced = journal.nextFillMs(accountId)
    if (paced > startMs) return { state: "paced", nextAtMs: paced }
    const held = holdEnd(params.holds, accountId, POOL_HOLD_GROUPS, startMs)
    if (held !== null) {
      journal.paceFill(accountId, held)
      return { state: "held", nextAtMs: held }
    }
    let status: PoolStatusReply
    try {
      status = await params.cloud.poolStatus(accountId)
    } catch (error) {
      const nextAtMs = startMs + POOL_STATUS_RECHECK_MS
      journal.paceFill(accountId, nextAtMs)
      return { state: "error", error: safeCloudError(error), nextAtMs }
    }
    params.cap.observe(status.cap, startMs)
    if (!status.allowedNow) {
      const nextAtMs = startMs + POOL_STATUS_RECHECK_MS
      journal.paceFill(accountId, nextAtMs)
      return { state: "not_allowed", reason: status.reason ?? null, nextAtMs }
    }
    // Deal Desk counts adds from the last report; the local count is newer.
    if (!capLeft(startMs)) {
      const nextAtMs = nextUtcDay(startMs)
      journal.paceFill(accountId, nextAtMs)
      return { state: "capped", nextAtMs }
    }
    if (params.signal?.aborted) return { state: "idle" }
    fill = journal.start({
      ref: (params.newRef ?? newPoolRef)(),
      accountId,
      nowMs: startMs,
    })
    fresh = true
  }
  let current: PoolFill = fill
  let step: FillStep = "create"
  const save = (update: PoolUpdate): PoolFill =>
    (current = journal.update(current.ref, update, now()))
  const paceNext = (fromMs: number) =>
    journal.paceFill(
      accountId,
      fromMs + jitteredGapMs(params.pacing, params.random),
    )
  const fail = async (error: unknown, code?: string): Promise<Stop> => {
    const text = `${step}:${code ?? safeProvisionError(error)}`
    save({ status: "failed", lastError: text })
    // The report is best effort; the local record keeps blocking fills.
    await params.cloud
      .poolFail({
        accountId: String(accountId),
        ...(current.chatId !== null ? { chatId: String(current.chatId) } : {}),
        stage: FAIL_STAGE[step],
        error: code ?? safeProvisionError(error),
        floodWaitS: floodWaitSeconds(error),
      })
      .catch(() => {})
    return new Stop({ state: "failed", ...at(current), error: text })
  }
  /** Classifies an error from `step`: wait, retry, or fail. */
  const stopFor = async (error: unknown): Promise<Stop> => {
    if (error instanceof Stop) return error
    const nowMs = now()
    const code = safeProvisionError(error)
    if (params.signal?.aborted)
      return new Stop({ state: "idle", ...at(current), error: code })
    if (isFloodOrHold(error)) {
      // The guard saved the hold; the fill and the next one wait it out.
      const retryAt = provisionRetryAt(error, nowMs)
      const until = retryAt ? Date.parse(retryAt) : nowMs + POOL_RETRY_MS
      save({ nextAttemptMs: until, lastError: `${step}:${code}` })
      journal.paceFill(accountId, until)
      return new Stop({
        state: "held",
        ...at(current),
        error: `${step}:${code}`,
        nextAtMs: until,
      })
    }
    if (isTransientTelegramError(error) || provisionRetryAt(error, nowMs)) {
      const attempts = current.attempts + 1
      if (attempts >= MAX_POOL_ATTEMPTS) {
        save({ attempts })
        return fail(error)
      }
      const nextAtMs = nowMs + POOL_RETRY_MS
      save({ attempts, nextAttemptMs: nextAtMs, lastError: `${step}:${code}` })
      return new Stop({
        state: "waiting",
        ...at(current),
        error: `${step}:${code}`,
        nextAtMs,
      })
    }
    return fail(error)
  }
  /** Passes a step: clears its attempts and error. */
  const passed = (update: PoolUpdate): PoolFill =>
    save({ ...update, attempts: 0, lastError: null })

  try {
    return await params.telegram(async (telegram) => {
      if ((await telegram.self()) !== accountId)
        throw new Error("Pool owner account identity mismatch")
      if (
        params.groups &&
        telegram.groupCount &&
        (!params.groups.value ||
          now() - params.groups.value.atMs >= GROUP_COUNT_INTERVAL_MS)
      ) {
        try {
          params.groups.value = {
            count: await telegram.groupCount(),
            atMs: now(),
          }
        } catch {
          // The count is informational; Deal Desk also counts owned chats.
        }
      }
      try {
        return await advance(telegram)
      } catch (error) {
        return (await stopFor(error)).result
      }
    })
  } catch (error) {
    // Session, connect, or identity errors before any step ran. A fresh
    // fill whose create was never sent made nothing: retire its ref.
    if (current.stage === "creating" && !createSent)
      return retireUnsent(safeProvisionError(error))
    const stop = await stopFor(error)
    return stop.result
  }

  /** Retires a fresh fill whose create call never left; nothing exists. */
  function retireUnsent(code: string): PoolFillResult {
    const nextAtMs = now() + POOL_RETRY_MS
    save({ status: "retired", lastError: `create_unsent:${code}` })
    journal.paceFill(accountId, nextAtMs)
    return { state: "retired", ...at(current), error: current.lastError!, nextAtMs }
  }

  async function advance(
    telegram: TelegramProvisioner,
  ): Promise<PoolFillResult> {
    const ref = current.ref
    if (current.stage === "creating" && !fresh) {
      // A create was journaled but its result never was: it may exist.
      save({ stage: "creation_unknown" })
    }
    if (current.stage === "creating") {
      step = "create"
      let chatId: number
      if (params.signal?.aborted) return retireUnsent("shutdown")
      createSent = true
      try {
        chatId = await telegram.create(POOL_CHAT_TITLE, poolDescription(ref))
      } catch (error) {
        if (params.signal?.aborted || !definiteCreateRejection(error)) {
          // A timeout may still have made the chat: find it by its ref.
          save({
            stage: "creation_unknown",
            nextAttemptMs: now() + POOL_RETRY_MS,
            lastError: `create:${safeProvisionError(error)}`,
          })
          paceNext(now())
          return {
            state: "waiting",
            ...at(current),
            error: current.lastError!,
            nextAtMs: current.nextAttemptMs,
          }
        }
        const retryAt = provisionRetryAt(error, now())
        if (retryAt) {
          // Refused before anything was made (a flood, a hold, or an
          // unreadable hold store): retire the ref and wait it out.
          const until = Date.parse(retryAt)
          const code = `create:${safeProvisionError(error)}`
          save({ status: "retired", lastError: code, nextAttemptMs: until })
          journal.paceFill(accountId, until)
          return { state: "held", ...at(current), error: code, nextAtMs: until }
        }
        throw await fail(error)
      }
      passed({ stage: "created", chatId })
      paceNext(now())
      // Deal Desk learns of the chat before the add; best effort, since
      // the local record resumes it either way.
      await params.cloud
        .poolRegister({
          accountId: String(accountId),
          chatId: String(chatId),
          state: "filling",
          ref,
          title: POOL_CHAT_TITLE,
          botUserId: String(PROVISIONING_BOT_ID),
          photoSet: false,
        })
        .catch(() => {})
    }
    if (current.stage === "creation_unknown") {
      step = "find_created"
      if (!telegram.poolCandidates)
        throw await fail(null, "candidate_search_unavailable")
      const found = await telegram.poolCandidates(ref)
      if (found.length > 1) throw await fail(null, "duplicate_pool_ref")
      if (!found.length) {
        const attempts = current.attempts + 1
        if (attempts >= MAX_CREATION_SEARCHES) {
          // Searched three times over 10+ minutes: nothing was created.
          save({
            status: "retired",
            attempts,
            lastError: "find_created:creation_not_found",
          })
          return { state: "retired", ...at(current), error: current.lastError! }
        }
        save({ attempts, nextAttemptMs: now() + POOL_RETRY_MS })
        return {
          state: "waiting",
          ...at(current),
          error: "find_created:creation_unknown",
          nextAtMs: current.nextAttemptMs,
        }
      }
      passed({ stage: "created", chatId: found[0]! })
    }
    const chatId = current.chatId
    if (chatId === null) throw await fail(null, "chat_missing")
    step = "verify_chat"
    if (!telegram.chatInfo) throw await fail(null, "chat_info_unavailable")
    const info = await telegram.chatInfo(chatId)
    const creator =
      info.isCreator || !telegram.creatorId
        ? null
        : await telegram.creatorId(chatId)
    if (!poolChatOwned(info, ref, params.owners, accountId, creator))
      throw await fail(null, "pool_chat_mismatch")

    if (current.stage === "created") {
      step = "read_bot"
      let bot =
        current.botAdds > 0
          ? await telegram.member(chatId, PROVISIONING_BOT_ID)
          : null
      if (current.botAdds === 0) {
        // Re-checked here: a resumed fill may reach its add on a new day
        // or after a hold began.
        const nowMs = now()
        const held = holdEnd(params.holds, accountId, ["invite"], nowMs)
        if (held !== null) {
          save({ nextAttemptMs: held })
          return { state: "held", ...at(current), nextAtMs: held }
        }
        if (!capLeft(nowMs)) {
          save({ nextAttemptMs: nextUtcDay(nowMs) })
          return {
            state: "capped",
            ...at(current),
            nextAtMs: current.nextAttemptMs,
          }
        }
        step = "add_bot"
        if (params.signal?.aborted) return { state: "idle", ...at(current) }
        // Journaled first: a lost response never leads to a second add.
        save({ botAdds: 1 })
        const record = (outcome: "added" | "refused" | "uncertain") => {
          try {
            params.queue.recordAdds({
              source: "pool",
              outcome,
              chatId,
              userIds: [PROVISIONING_BOT_ID],
              nowMs: now(),
            })
          } catch {
            // The add happened; a missed count only undercounts today.
          }
        }
        try {
          await telegram.add(chatId, PROVISIONING_BOT_ID)
        } catch (error) {
          if (isTransientTelegramError(error)) {
            // Telegram may have run it: counted, and never sent again.
            record("uncertain")
            paceNext(now())
          } else if (isPerUserAddError(error)) {
            record("refused")
            paceNext(now())
          } else if (
            !params.signal?.aborted &&
            provisionRetryAt(error, now()) !== null
          ) {
            // A flood or hold refused the whole call: nothing was sent.
            save({ botAdds: 0 })
          }
          throw error
        }
        record("added")
        paceNext(now())
        step = "read_bot"
        bot = await telegram.member(chatId, PROVISIONING_BOT_ID)
        if (bot.role === "absent") {
          await (params.sleep ?? defaultSleep)(MEMBER_RECHECK_MS)
          bot = await telegram.member(chatId, PROVISIONING_BOT_ID)
        }
      }
      if (!bot || bot.role === "absent") {
        // One add per pool chat: an absent bot is read again, never re-added.
        const attempts = current.attempts + 1
        if (attempts >= MAX_BOT_VISIBILITY_CHECKS) {
          save({ attempts })
          throw await fail(null, "TG_SAI_BOT_NOT_VISIBLE")
        }
        save({
          attempts,
          nextAttemptMs: now() + POOL_BOT_RECHECK_MS,
          lastError: "read_bot:TG_SAI_BOT_NOT_VISIBLE",
        })
        return {
          state: "waiting",
          ...at(current),
          error: current.lastError!,
          nextAtMs: current.nextAttemptMs,
        }
      }
      passed({ stage: "bot_added" })
    }

    let bot: Member | null = null
    if (current.stage === "bot_added") {
      step = "promote_bot"
      const promote = telegram.promoteOnce ?? telegram.promote
      await promote.call(telegram, chatId, PROVISIONING_BOT_ID, {
        ...POOL_BOT_RIGHTS,
      })
      bot = await telegram.member(chatId, PROVISIONING_BOT_ID)
      if (!hasPoolBotRights(bot)) throw await fail(null, "bot_rights_missing")
      passed({ stage: "promoted" })
    }

    let photoSet = info.hasPhoto === true
    if (current.stage === "promoted") {
      step = "photo"
      if (!photoSet && telegram.setPhoto) {
        try {
          await telegram.setPhoto(chatId, await params.photo())
          photoSet = true
        } catch (error) {
          // A flood or transient error waits and retries; any other photo
          // error leaves the chat without one, since the photo is cosmetic.
          if (
            isFloodOrHold(error) ||
            isTransientTelegramError(error) ||
            params.signal?.aborted
          )
            throw error
          save({ lastError: `photo:${safeProvisionError(error)}` })
        }
      }
      save({ stage: "photo_set", attempts: 0 })
    }

    if (current.stage === "photo_set") {
      step = "register"
      // Deal Desk wants a verification within 5 minutes of registering.
      bot ??= await telegram.member(chatId, PROVISIONING_BOT_ID)
      if (!hasPoolBotRights(bot)) {
        step = "promote_bot"
        throw await fail(null, "bot_rights_missing")
      }
      if (!photoSet && telegram.chatInfo)
        photoSet = (await telegram.chatInfo(chatId)).hasPhoto === true
      try {
        await params.cloud.poolRegister({
          accountId: String(accountId),
          chatId: String(chatId),
          state: "available",
          ref,
          title: POOL_CHAT_TITLE,
          botUserId: String(PROVISIONING_BOT_ID),
          botRights: Object.fromEntries(
            Object.entries(bot.rights).filter(([, v]) => v === true),
          ),
          photoSet,
          verifiedAt: new Date(now()).toISOString(),
        })
      } catch (error) {
        // Deal Desk down or refusing: the chat is ready, so retry the
        // registration with backoff and never fail the chat for it.
        const attempts = current.attempts + 1
        const delay = Math.min(POOL_RETRY_MS * attempts, 30 * 60 * 1000)
        save({
          attempts,
          nextAttemptMs: now() + delay,
          lastError: `register:${safeCloudError(error)}`,
        })
        return {
          state: "waiting",
          ...at(current),
          error: current.lastError!,
          nextAtMs: current.nextAttemptMs,
        }
      }
      passed({ stage: "registered", status: "done" })
      return { state: "filled", ...at(current) }
    }
    return { state: "progress", ...at(current) }
  }
}

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** A Deal Desk error code without CLI output. */
function safeCloudError(error: unknown): string {
  const code = (error as { code?: unknown } | null)?.code
  return typeof code === "string" && /^[A-Z_]{1,64}$/.test(code)
    ? code
    : "deal_desk_failed"
}
