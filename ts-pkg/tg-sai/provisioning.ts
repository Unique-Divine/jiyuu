import { createHash } from "node:crypto"

import { HOLD_ACCOUNT_UNKNOWN } from "./state/guard"
import { holdUntil, PEER_FLOOD_WAIT_S } from "./state/holds"

export const PROVISIONING_BOT_ID = 8983431101
export type Peer = number | string
export interface ProvisionRequest {
  requestId: string
  custodyProfile: string
  expectedUserId: number
  title: string
  participants: Peer[]
  optionalParticipants?: Peer[]
  admins: { userId: Peer; rightsProfile: "team" }[]
  botRightsProfile: "baseline"
  dealIds: string[]
}
export type ProvisionState =
  | "prepared"
  | "creating"
  | "creation_unknown"
  | "configuring"
  | "incomplete"
  | "awaiting_join"
  | "ready"
  | "cancelled"
export interface Binding {
  participants: number[]
  admins: number[]
  optionalParticipants?: number[]
}
export interface Setup extends ProvisionRequest {
  state: ProvisionState
  chatId?: number
  binding?: Binding
  claimToken?: string
  receipt?: unknown
  retryAt?: string | null
  /** When an `awaiting_join` setup is due for its next check. */
  nextCheckAt?: string | null
  /** Last saved diagnostic, `<step>:<code>`, when the cloud reports one. */
  error?: string | null
}
export interface Member {
  role: "creator" | "admin" | "member" | "absent"
  rights: Record<string, boolean>
}
export interface ProvisioningLinks {
  openGroupUrl?: string
  primaryInviteLink?: string
}
export interface TelegramProvisioner {
  self(): Promise<number>
  resolve(peer: Peer): Promise<{ id: number; isBot: boolean }>
  create(title: string, marker: string): Promise<number>
  member(chatId: number, userId: number): Promise<Member>
  add(chatId: number, userId: number): Promise<void>
  /**
   * Adds several users in one call. Resolves to the IDs Telegram reports as
   * not invited (privacy); throws for errors that refuse the whole call.
   */
  addMany?(chatId: number, userIds: number[]): Promise<number[]>
  inviteLink?(
    chatId: number,
    userId: number,
  ): Promise<{ inviteLink: string; expiresAt: string }>
  /**
   * Approves a pending join request from `userId`, as sent through a
   * controlled invitation. Resolves `false` when that user has no pending
   * request. Approval is not a member add.
   */
  approveJoin?(chatId: number, userId: number): Promise<boolean>
  /** Read existing links only. Failure must not invalidate verified setup. */
  completionLinks?(chatId: number): Promise<ProvisioningLinks>
  promote(
    chatId: number,
    userId: number,
    rights: Record<string, boolean>,
  ): Promise<void>
  candidates(marker: string): Promise<number[]>
  /**
   * Every participant in one listing, keyed by user ID. Absent users are
   * missing from the map.
   */
  members?(chatId: number): Promise<Map<number, Member>>
  verifyMarker(chatId: number, marker: string): Promise<boolean>
}
export interface Progress {
  state: ProvisionState
  chatId?: number
  binding?: Binding
  receipt?: unknown
  error?: string
  creationRejected?: boolean
  retryAt?: string | null
  /** Required with `awaiting_join`: when the worker checks the chat again. */
  nextCheckAt?: string
}
export interface ProvisionCloud {
  view(requestId: string): Promise<Setup>
  claim(requestId: string, workerId: string): Promise<Setup>
  progress(
    requestId: string,
    claimToken: string,
    progress: Progress,
  ): Promise<Setup>
  resolve(requestId: string, chatId: number, reason: string): Promise<Setup>
}
export interface JournalRecord {
  fingerprint: string
  state: ProvisionState
  chatId?: number
  binding?: Binding
  workerId?: string
  pendingInvitations?: PendingInvitation[]
  /** Bot add calls sent and not refused; bounds a bot that never shows. */
  botAdds?: number
  /** Runs that ended in a transient error; bounds unattended retries. */
  transientRetries?: number
  /**
   * People an add call reached, or may have reached (a timeout). A resume
   * that finds one absent gives them a link, never another add.
   */
  addedIds?: number[]
  /**
   * Runs in a row that stopped on a transient error at a step that spends
   * no add, and that step; bounds how long such a setup holds the account.
   */
  stalledRuns?: number
  stalledStep?: string
}
export interface PendingInvitation {
  userId: string
  inviteLink: string
  expiresAt: string
  reason: string
}
/** Where optional members of a verified chat wait to be added. */
export interface BackgroundEnqueue {
  enqueue(input: {
    requestId: string
    chatId: number
    userIds: number[]
  }): void | Promise<void>
}
/** `reason` of the shared invitation recorded for background members. */
export const BACKGROUND_INVITATION_REASON = "background_queue"
/** A saved invitation is reused only while it has this much time left. */
const MIN_INVITATION_LEAD_MS = 10 * 60 * 1000
export interface ProvisionJournal {
  read(requestId: string): Promise<JournalRecord | null>
  write(requestId: string, record: JournalRecord): Promise<void>
}

const rightsOff = {
  changeInfo: false,
  postMessages: false,
  editMessages: false,
  deleteMessages: false,
  banUsers: false,
  inviteUsers: false,
  pinMessages: false,
  addAdmins: false,
  anonymous: false,
  manageCall: false,
  other: false,
  manageTopics: false,
  postStories: false,
  editStories: false,
  deleteStories: false,
  manageDirectMessages: false,
  manageRanks: false,
}
export const BOT_BASELINE_RIGHTS = {
  ...rightsOff,
  // Telegram enables basic chat management for every administrator.
  other: true,
  inviteUsers: true,
  pinMessages: true,
}
export const TEAM_ADMIN_RIGHTS = {
  ...rightsOff,
  other: true,
  changeInfo: true,
  inviteUsers: true,
  pinMessages: true,
}

export function validateRequest(value: ProvisionRequest): ProvisionRequest {
  if (!value || !/^[A-Za-z0-9:_-]{1,128}$/.test(value.requestId))
    throw new Error("Invalid requestId")
  if (
    typeof value.custodyProfile !== "string" ||
    !value.custodyProfile.trim() ||
    value.custodyProfile.length > 64
  )
    throw new Error("Explicit custodyProfile required")
  if (
    !Number.isSafeInteger(value.expectedUserId) ||
    value.expectedUserId <= 0 ||
    value.expectedUserId === PROVISIONING_BOT_ID
  )
    throw new Error("Invalid expectedUserId")
  if (
    typeof value.title !== "string" ||
    !value.title.trim() ||
    Array.from(value.title).length > 128
  )
    throw new Error("Nonblank title required")
  for (const key of ["participants", "admins"] as const) {
    if (
      !Array.isArray(value[key]) ||
      value[key].length > (key === "participants" ? 100 : 30) ||
      value[key].some((entry) => {
        const peer =
          key === "admins"
            ? (entry as { userId: Peer; rightsProfile: string })?.userId
            : entry
        if (
          key === "admins" &&
          (entry as { rightsProfile: string })?.rightsProfile !== "team"
        )
          return true
        return typeof peer === "number"
          ? !Number.isSafeInteger(peer) || peer <= 0
          : typeof peer !== "string" ||
              !/^(?:[1-9][0-9]*|@?[A-Za-z][A-Za-z0-9_]{4,31})$/.test(peer)
      })
    )
      throw new Error(`Invalid ${key}`)
  }
  if (
    value.optionalParticipants !== undefined &&
    (!Array.isArray(value.optionalParticipants) ||
      value.optionalParticipants.length > 100 ||
      new Set(value.optionalParticipants.map(String)).size !==
        value.optionalParticipants.length ||
      value.optionalParticipants.some(
        (peer) =>
          !value.participants.some((p) => String(p) === String(peer)) ||
          value.admins.some((a) => String(a.userId) === String(peer)),
      ))
  )
    throw new Error(
      "Optional participants must be distinct ordinary requested members",
    )
  if (value.botRightsProfile !== "baseline")
    throw new Error("Unknown botRightsProfile; use baseline")
  if (
    !Array.isArray(value.dealIds) ||
    value.dealIds.length > 50 ||
    value.dealIds.some((id) => typeof id !== "string" || !id.trim()) ||
    new Set(value.dealIds).size !== value.dealIds.length
  )
    throw new Error("Invalid or duplicate dealIds")
  return value
}
export function requestFingerprint(request: ProvisionRequest): string {
  const value = validateRequest(request)
  return createHash("sha256")
    .update(
      JSON.stringify({
        requestId: value.requestId,
        custodyProfile: value.custodyProfile,
        expectedUserId: value.expectedUserId,
        title: value.title,
        participants: value.participants,
        ...(value.optionalParticipants === undefined
          ? {}
          : { optionalParticipants: value.optionalParticipants }),
        admins: value.admins,
        botRightsProfile: value.botRightsProfile,
        dealIds: value.dealIds,
      }),
    )
    .digest("hex")
}
export function creationMarker(request: ProvisionRequest): string {
  return `sai-setup:${requestFingerprint(request)}`
}
function validBinding(binding: Binding, ownerId: number): boolean {
  return (
    Array.isArray(binding.participants) &&
    Array.isArray(binding.admins) &&
    [...binding.participants, ...binding.admins].every(
      (id) =>
        Number.isSafeInteger(id) &&
        id > 0 &&
        id !== PROVISIONING_BOT_ID &&
        id !== ownerId,
    ) &&
    binding.admins.every((id) => binding.participants.includes(id)) &&
    (binding.optionalParticipants === undefined ||
      (Array.isArray(binding.optionalParticipants) &&
        binding.optionalParticipants.every(
          (id) =>
            binding.participants.includes(id) && !binding.admins.includes(id),
        ) &&
        new Set(binding.optionalParticipants).size ===
          binding.optionalParticipants.length)) &&
    new Set(binding.participants).size === binding.participants.length &&
    new Set(binding.admins).size === binding.admins.length
  )
}
function hasRights(member: Member, expected: Record<string, boolean>): boolean {
  return (
    member.role === "admin" &&
    Object.entries(expected).every(
      ([key, value]) => Boolean(member.rights[key]) === value,
    )
  )
}
export async function bindRequest(
  request: ProvisionRequest,
  telegram: TelegramProvisioner,
): Promise<Binding> {
  const ids = new Map<Peer, number>()
  for (const peer of [
    ...request.participants,
    ...request.admins.map((admin) => admin.userId),
  ]) {
    if (ids.has(peer)) continue
    const resolved = await telegram.resolve(peer)
    if (
      resolved.isBot ||
      !Number.isSafeInteger(resolved.id) ||
      resolved.id <= 0
    )
      throw new Error(
        "Participants and team admins must resolve to user accounts",
      )
    ids.set(peer, resolved.id)
  }
  const admins = request.admins.map((admin) => ids.get(admin.userId)!)
  const participants = request.participants.map((peer) => ids.get(peer)!)
  const binding = {
    participants,
    admins,
    ...(request.optionalParticipants === undefined
      ? {}
      : {
          optionalParticipants: request.optionalParticipants.map(
            (peer) => ids.get(peer)!,
          ),
        }),
  }
  if (!validBinding(binding, request.expectedUserId))
    throw new Error("Invalid account binding")
  return binding
}

/** Only explicit server rejection proves creation did not happen. Transport failures remain unknown. */
export function definiteCreateRejection(error: unknown): boolean {
  const e = error as { text?: string; code?: number }
  if (typeof e?.text !== "string") return false
  // The local hold guard refuses before sending, so the call never reached
  // Telegram: a hold (420), an unreadable hold store, or an unknown session
  // account (500).
  if (
    (e.code === 420 && /^TG_SAI_HOLD_\d+$/.test(e.text)) ||
    (e.code === 500 &&
      (e.text === HOLD_STATE_UNAVAILABLE || e.text === HOLD_ACCOUNT_UNKNOWN))
  )
    return true
  return (
    [400, 401, 403, 420].includes(e.code ?? 0) &&
    /^(?:FLOOD_WAIT|FLOOD_PREMIUM_WAIT|PEER_FLOOD|CHANNELS_TOO_MUCH|USER_RESTRICTED|CHAT_TITLE_EMPTY|CHAT_ABOUT_TOO_LONG|AUTH_KEY_UNREGISTERED|SESSION_REVOKED)(?:_%d|_\d+)?$/.test(
      e.text,
    )
  )
}
export function safeProvisionError(error: unknown): string {
  const e = error as { text?: string; errorMessage?: string; seconds?: number }
  const code = e?.text ?? e?.errorMessage
  const normalized =
    typeof code === "string"
      ? code.replace(
          "%d",
          String(Number.isSafeInteger(e.seconds) ? e.seconds : "UNKNOWN"),
        )
      : ""
  if (isTransientTelegramError(error)) return TELEGRAM_TRANSIENT
  return /^(?:FLOOD(?:_PREMIUM)?_WAIT_(?:\d{1,10}|UNKNOWN)|PEER_FLOOD|TG_SAI_HOLD_\d{1,10}|TG_SAI_HOLD_STATE_UNAVAILABLE|TG_SAI_HOLD_ACCOUNT_UNKNOWN|TG_SAI_MEMBER_NOT_VISIBLE|TG_SAI_INVITATION_EXPIRED|TG_SAI_BOT_NOT_VISIBLE|TG_SAI_BOT_ADDS_EXHAUSTED|TG_SAI_TRANSIENT_LIMIT|TG_SAI_AWAITING_JOIN|TG_SAI_QUEUE_UNAVAILABLE|USER_PRIVACY_RESTRICTED|USER_NOT_MUTUAL_CONTACTS?|USER_NOT_PARTICIPANT|USER_CHANNELS_TOO_MUCH|USER_BOT|USER_ID_INVALID|PEER_ID_INVALID|CHAT_ID_INVALID|CHANNEL_INVALID|CHANNEL_PRIVATE|CHAT_ADMIN_REQUIRED|CHAT_WRITE_FORBIDDEN|USER_ADMIN_INVALID|USER_CREATOR|ADMINS_TOO_MUCH|BOTS_TOO_MUCH|BOT_GROUPS_BLOCKED|RIGHT_FORBIDDEN|USER_RESTRICTED|USER_DEACTIVATED|INPUT_USER_DEACTIVATED|CHANNELS_TOO_MUCH|USERS_TOO_MUCH|AUTH_KEY_UNREGISTERED|SESSION_REVOKED|INVITE_HASH_EXPIRED|INVITE_HASH_INVALID|INVITE_REQUEST_SENT)$/.test(
    normalized,
  )
    ? normalized
    : "provisioning_step_failed"
}

type ProvisionStep =
  | "verify_marker"
  | "save_configuring"
  | "read_member"
  | "add_member"
  | "verify_added_member"
  | "approve_join"
  | "create_invitation"
  | "save_invitation"
  | "read_admin"
  | "promote_admin"
  | "read_bot"
  | "promote_bot"
  | "audit_membership"
  | "enqueue_background"
  | "save_ready"

/** Steps in the order a run reaches them, to tell when a run got further. */
const STEP_ORDER: readonly ProvisionStep[] = [
  "verify_marker",
  "save_configuring",
  "read_member",
  "approve_join",
  "add_member",
  "verify_added_member",
  "create_invitation",
  "save_invitation",
  "read_admin",
  "promote_admin",
  "read_bot",
  "promote_bot",
  "audit_membership",
  "enqueue_background",
  "save_ready",
]

/** Fixed operation names add context without copying Telegram response bodies. */
export function safeProvisionDiagnostic(
  error: unknown,
  step: ProvisionStep,
): string {
  return `${step}:${safeProvisionError(error)}`
}

/**
 * Whether a refused add concerns only the user(s) being added, so the other
 * users can still be tried one by one: privacy refusals, and 400 errors
 * named `USER_*`, `CHAT_*`, or `BOT_GROUPS_BLOCKED` (e.g.
 * `USER_CHANNELS_TOO_MUCH`). Flood waits, `PEER_FLOOD`, and local holds
 * are never per-user.
 */
export function isPerUserAddError(error: unknown): boolean {
  if (isPrivacyRestriction(error)) return true
  const e = error as { text?: unknown; errorMessage?: unknown; code?: unknown }
  const text = typeof e?.text === "string" ? e.text : e?.errorMessage
  return (
    e?.code === 400 &&
    typeof text === "string" &&
    /^(?:USER_[A-Z0-9_]+|CHAT_[A-Z0-9_]+|BOT_GROUPS_BLOCKED)$/.test(text)
  )
}

export function isPrivacyRestriction(error: unknown): boolean {
  const e = error as { text?: string; errorMessage?: string; code?: number }
  return [e?.text, e?.errorMessage].some(
    (value) =>
      typeof value === "string" &&
      /^(USER_PRIVACY_RESTRICTED|USER_NOT_MUTUAL_CONTACT|USER_NOT_MUTUAL_CONTACTS)$/.test(
        value,
      ),
  )
}
/** Text of `HoldStoreUnavailableError` (state/holds.ts). */
export const HOLD_STATE_UNAVAILABLE = "TG_SAI_HOLD_STATE_UNAVAILABLE"
/** Retry delay while the hold store cannot be read. */
export const HOLD_STATE_UNAVAILABLE_RETRY_MS = 5 * 60 * 1000
/**
 * Code for a Telegram error that says nothing about the request: a
 * timeout, a dropped or destroyed connection, or a Telegram server error.
 * The setup retries after `TRANSIENT_RETRY_MS`.
 */
export const TELEGRAM_TRANSIENT = "TG_SAI_TRANSIENT"
/** Retry delay after a transient Telegram error. */
export const TRANSIENT_RETRY_MS = 5 * 60 * 1000
/** Code for a failed write to the local background queue (`state.db`). */
export const QUEUE_UNAVAILABLE = "TG_SAI_QUEUE_UNAVAILABLE"

const TRANSIENT_TEXT =
  /^(?:TIMEOUT|Timeout|Timedout|RPC_CALL_FAIL|RPC_MCGET_FAIL|INTERNAL|INTERNAL_SERVER_ERROR|WORKER_BUSY_TOO_LONG_RETRY|MSG_WAIT_FAILED|MSG_WAIT_TIMEOUT|INTERDC_(?:\d+|%d)_CALL_(?:RICH_)?ERROR)$/
// mtcute's `MtTimeoutError`, `TransportError`, and connection errors.
const TRANSIENT_MESSAGE =
  /^(?:Request timed out|Transport error: |Connection closed$|Connection destroyed$|Not connected to any DC$|Client is destroyed$)/
const TRANSIENT_ERRNO =
  /^(?:ECONNRESET|ECONNREFUSED|ECONNABORTED|ETIMEDOUT|EPIPE|ENETDOWN|ENETUNREACH|EHOSTUNREACH|EAI_AGAIN)$/

/**
 * Whether `error` is a temporary Telegram or network state that says
 * nothing about the request: a Telegram server error (5xx or negative
 * code, such as `-503 Timeout`), a timeout, a dropped connection, or a
 * client destroyed at shutdown. Local `TG_SAI_*` refusals and flood waits
 * are not transient here; they carry their own retry time.
 */
export function isTransientTelegramError(error: unknown): boolean {
  const e = error as {
    text?: unknown
    errorMessage?: unknown
    code?: unknown
    message?: unknown
    name?: unknown
  }
  if (!e || typeof e !== "object") return false
  const text = typeof e.text === "string" ? e.text : e.errorMessage
  if (typeof text === "string") {
    if (text.startsWith("TG_SAI_")) return false
    if (TRANSIENT_TEXT.test(text)) return true
  }
  if (typeof e.code === "number" && (e.code >= 500 || e.code < 0)) return true
  if (typeof e.code === "string" && TRANSIENT_ERRNO.test(e.code)) return true
  if (e.name === "AbortError" || e.name === "TimeoutError") return true
  return typeof e.message === "string" && TRANSIENT_MESSAGE.test(e.message)
}
/**
 * Code for a member whose add call succeeded but who is still not in the
 * chat after one re-check. The member gets a controlled invitation and the
 * setup retries after `MEMBER_NOT_VISIBLE_RETRY_MS` without adding again.
 */
export const MEMBER_NOT_VISIBLE = "TG_SAI_MEMBER_NOT_VISIBLE"
/**
 * Code for a member whose `added_not_visible` invitation expired before they
 * joined. The setup is saved without `retryAt`, so it needs an operator; the
 * worker never adds that member again.
 */
export const INVITATION_EXPIRED = "TG_SAI_INVITATION_EXPIRED"
/** Retry delay while an added member is not yet visible. */
export const MEMBER_NOT_VISIBLE_RETRY_MS = 15 * 60 * 1000
/** Wait before re-reading a member whose add succeeded but who is absent. */
export const MEMBER_RECHECK_MS = 3000
/** `reason` of the invitation for a member added but not yet visible. */
export const NOT_VISIBLE_INVITATION_REASON = "added_not_visible"
/**
 * Code for a bot whose add succeeded but who is not in the chat yet. The bot
 * cannot use a link, so the resumed job adds it again if it is still absent;
 * retries after `MEMBER_NOT_VISIBLE_RETRY_MS`. Not an invitation wait.
 */
export const BOT_NOT_VISIBLE = "TG_SAI_BOT_NOT_VISIBLE"
/**
 * Code for a bot still absent once `MAX_BOT_ADDS` add calls were sent. Saved
 * without `retryAt`, so it needs an operator; no further bot add runs.
 */
export const BOT_ADDS_EXHAUSTED = "TG_SAI_BOT_ADDS_EXHAUSTED"
/** Bot add calls per setup: the first add and one re-add. */
export const MAX_BOT_ADDS = 2
/**
 * Code for a setup whose runs ended in a transient error
 * `MAX_TRANSIENT_RETRIES` times. Saved without `retryAt`, so it needs an
 * operator: a stuck setup cannot keep the account busy or keep adding.
 */
export const TRANSIENT_LIMIT = "TG_SAI_TRANSIENT_LIMIT"
/** Transient failures a setup may hit before it stops for an operator. */
export const MAX_TRANSIENT_RETRIES = 3
/**
 * Runs in a row a setup may stop on a transient error at a step that spends
 * no add (a read, approval, promotion, or link) before it stops for an
 * operator with `<step>:TG_SAI_TRANSIENT_LIMIT`: about an hour of 5-minute
 * retries.
 */
export const MAX_STALLED_RUNS = 12
/**
 * Code for a setup that waits only for required people whose add was
 * refused for privacy and who got a controlled invitation instead. Retries
 * after `MEMBER_NOT_VISIBLE_RETRY_MS`.
 */
export const AWAITING_JOIN = "TG_SAI_AWAITING_JOIN"
/** `reason` of the invitation for a required person refused for privacy. */
export const PRIVACY_INVITATION_REASON = "privacy_restricted"
/** Invitations a resumed setup waits on instead of adding again. */
const WAIT_INVITATION_REASONS = new Set([
  NOT_VISIBLE_INVITATION_REASON,
  PRIVACY_INVITATION_REASON,
])

/**
 * Whether a saved setup is waiting for an invited member to join: state
 * `awaiting_join`, or an `incomplete` row saved with one of the wait codes
 * before Deal Desk had that state. Such a setup makes no add while it
 * waits, holds no Deal Desk claim, and never makes the account busy.
 */
export function waitingOnInvitedMember(setup: {
  state?: ProvisionState
  error?: string | null
}): boolean {
  // Every step that saves `TG_SAI_MEMBER_NOT_VISIBLE` first gives the
  // people it waits on a link; the bot has its own code.
  return (
    setup.state === "awaiting_join" ||
    setup.error?.endsWith(`:${MEMBER_NOT_VISIBLE}`) === true ||
    setup.error === `add_member:${AWAITING_JOIN}`
  )
}
/** Telegram gives no duration for `PEER_FLOOD`; holds use 24 h. */
export const PEER_FLOOD_RETRY_MS = PEER_FLOOD_WAIT_S * 1000
/**
 * Least lead a cloud `retryAt` gets over now, so Deal Desk never rejects
 * it as not in the future after clock skew or a slow request.
 */
export const MIN_CLOUD_RETRY_MS = 30_000
const MAX_RETRY_MS = 31_536_000 * 1000

/**
 * Cloud `retryAt` for a local hold ending at `untilMs`: that time, or
 * `MIN_CLOUD_RETRY_MS` from now when later. Never before the hold ends.
 * `null` when the time is not finite or more than a year out.
 */
export function cloudRetryAt(untilMs: number, now = Date.now()): string | null {
  if (!Number.isFinite(untilMs)) return null
  const at = Math.max(Math.ceil(untilMs), now + MIN_CLOUD_RETRY_MS)
  return at - now <= MAX_RETRY_MS ? new Date(at).toISOString() : null
}

/**
 * When a flood or hold error allows an unattended retry: `FLOOD_WAIT_N`
 * (also mtcute's `FLOOD_WAIT_%d` with `seconds`), `FLOOD_PREMIUM_WAIT_N`,
 * `PEER_FLOOD` (24 h), `TG_SAI_HOLD_N` (the hold's `untilMs` when present),
 * `TG_SAI_HOLD_STATE_UNAVAILABLE`, `TG_SAI_HOLD_ACCOUNT_UNKNOWN`, and
 * `TG_SAI_QUEUE_UNAVAILABLE` (5 min), `TG_SAI_MEMBER_NOT_VISIBLE`,
 * `TG_SAI_BOT_NOT_VISIBLE`, and `TG_SAI_AWAITING_JOIN` (15 min), and transient Telegram errors
 * (`isTransientTelegramError`, 5 min). Telegram waits get the same margin
 * as the local hold (`holdUntil`), and every time is at least
 * `MIN_CLOUD_RETRY_MS` out (`cloudRetryAt`). `null` for anything else.
 */
export function provisionRetryAt(
  error: unknown,
  now = Date.now(),
): string | null {
  const e = error as {
    text?: string
    errorMessage?: string
    seconds?: number
    untilMs?: number
  }
  const code = e?.text ?? e?.errorMessage ?? ""
  const after = (waitS: number): string | null =>
    Number.isSafeInteger(waitS) && waitS > 0
      ? cloudRetryAt(holdUntil(now, waitS), now)
      : null
  if (code === "PEER_FLOOD") return after(PEER_FLOOD_WAIT_S)
  if (
    code === HOLD_STATE_UNAVAILABLE ||
    code === HOLD_ACCOUNT_UNKNOWN ||
    code === QUEUE_UNAVAILABLE
  )
    return cloudRetryAt(now + HOLD_STATE_UNAVAILABLE_RETRY_MS, now)
  if (
    code === MEMBER_NOT_VISIBLE ||
    code === BOT_NOT_VISIBLE ||
    code === AWAITING_JOIN
  )
    return cloudRetryAt(now + MEMBER_NOT_VISIBLE_RETRY_MS, now)
  const hold = /^TG_SAI_HOLD_(\d+)$/.exec(code)
  if (hold) {
    if (Number.isSafeInteger(e.untilMs) && e.untilMs! > now)
      return cloudRetryAt(e.untilMs!, now)
    const seconds = Number(hold[1])
    return Number.isSafeInteger(seconds) && seconds > 0
      ? cloudRetryAt(now + seconds * 1000, now)
      : null
  }
  const match = /^FLOOD(?:_PREMIUM)?_WAIT_(\d+|%d)$/.exec(code)
  if (match)
    return after(Number(match[1] === "%d" ? e.seconds : match[1]))
  return isTransientTelegramError(error)
    ? cloudRetryAt(now + TRANSIENT_RETRY_MS, now)
    : null
}

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
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

/** A member Telegram accepted but does not show yet; retried later. */
function notVisibleError(): Error {
  return Object.assign(new Error("Added member not visible yet"), {
    text: MEMBER_NOT_VISIBLE,
  })
}

/** The bot was accepted but does not show yet; retried, re-added once. */
function botNotVisibleError(): Error {
  return Object.assign(new Error("Added bot not visible yet"), {
    text: BOT_NOT_VISIBLE,
  })
}

/**
 * Creates a controlled invitation for `userId` and checks its shape: a
 * `t.me` invite URL with a future expiry.
 */
async function controlledInvitation(
  telegram: TelegramProvisioner,
  chatId: number,
  userId: number,
): Promise<{ inviteLink: string; expiresAt: string }> {
  if (!telegram.inviteLink) throw new Error("Invitation links unavailable")
  const created = await telegram.inviteLink(chatId, userId)
  if (
    !/^https:\/\/t\.me\/(?:\+|joinchat\/)[A-Za-z0-9_-]+$/.test(
      created.inviteLink,
    ) ||
    !Number.isFinite(Date.parse(created.expiresAt)) ||
    Date.parse(created.expiresAt) <= Date.now()
  )
    throw new Error("Invalid controlled invitation link")
  return created
}

/**
 * The cloud is authoritative. The journal only recovers a result not yet
 * acknowledged by cloud. Requester first: the job adds required participants
 * and the bot, promotes, verifies, and reports ready; optional participants
 * still absent go to `background` and share one pending invitation.
 *
 * A person whose add succeeded but who is still absent after one re-check
 * gets a controlled invitation instead of a failed setup. The setup stays
 * `incomplete` with a `retryAt`, and a resumed job never adds that person
 * again: it approves their join request, if one is waiting, and otherwise
 * waits. Once the invitation expires, the setup is saved without `retryAt`
 * (`TG_SAI_INVITATION_EXPIRED`) for an operator.
 */
export async function provisionSetup(params: {
  setup: Setup
  telegram: TelegramProvisioner
  cloud: ProvisionCloud
  journal: ProvisionJournal
  workerId: string
  /** Queue for optional members; required when the request has any. */
  background?: BackgroundEnqueue
  /** Waits before a membership re-check; tests pass a no-op. */
  sleep?: (ms: number) => Promise<void>
  /** Worker shutdown; a run it cuts short is not a transient failure. */
  signal?: AbortSignal
}): Promise<Setup | { state: "creation_unknown"; candidates: number[] }> {
  const { telegram, cloud, journal } = params
  let setup = params.setup
  const fingerprint = requestFingerprint(setup)
  if (setup.state === "cancelled") throw new Error("Setup is cancelled")
  if ((await telegram.self()) !== setup.expectedUserId)
    throw new Error("Custody account identity mismatch")
  const local = await journal.read(setup.requestId)
  if (local && local.fingerprint !== fingerprint)
    throw new Error("Local receipt fingerprint conflicts with cloud request")
  if (setup.state === "ready") {
    if (
      !setup.chatId ||
      !(await telegram.verifyMarker(setup.chatId, creationMarker(setup)))
    )
      throw new Error("Ready setup owner or marker changed")
    const owner = await telegram.member(setup.chatId, setup.expectedUserId)
    const bot = await telegram.member(setup.chatId, PROVISIONING_BOT_ID)
    if (owner.role !== "creator" || !hasRights(bot, BOT_BASELINE_RIGHTS))
      throw new Error("Ready setup rights changed")
    if (setup.binding)
      for (const id of setup.binding.admins)
        if (
          !hasRights(await telegram.member(setup.chatId, id), TEAM_ADMIN_RIGHTS)
        )
          throw new Error("Ready setup admin rights changed")
    return setup
  }
  // Checked before any write, so a chat is never created without one.
  if (setup.optionalParticipants?.length && !params.background)
    throw new Error("Background member queue required")
  let pendingInvitations = local?.pendingInvitations ?? []
  let botAdds = local?.botAdds ?? 0
  let transientRetries = local?.transientRetries ?? 0
  let addedIds = new Set(local?.addedIds ?? [])
  let stalledRuns = local?.stalledRuns ?? 0
  let stalledStep = local?.stalledStep
  interface Counts {
    botAdds: number
    transientRetries: number
    addedIds: Set<number>
  }
  // Counters every journal write carries, so none is lost on restart. A
  // change is written first and kept in memory only once the write worked.
  const counts = (next: Partial<Counts> = {}) => {
    const c = { botAdds, transientRetries, addedIds, ...next }
    return {
      ...(c.botAdds ? { botAdds: c.botAdds } : {}),
      ...(c.transientRetries ? { transientRetries: c.transientRetries } : {}),
      ...(c.addedIds.size ? { addedIds: [...c.addedIds] } : {}),
      ...(stalledRuns ? { stalledRuns, stalledStep } : {}),
    }
  }
  const workerId = local?.workerId ?? params.workerId
  if (
    !setup.chatId &&
    (setup.state === "creating" ||
      setup.state === "creation_unknown" ||
      local?.state === "creating" ||
      local?.state === "creation_unknown")
  ) {
    if (setup.state === "creating") {
      const claimed = await cloud.claim(setup.requestId, workerId)
      if (!claimed.claimToken)
        throw new Error("Cloud did not return a recovery claim")
      await cloud.progress(setup.requestId, claimed.claimToken, {
        state: "creation_unknown",
        binding: setup.binding,
      })
    }
    // Even a unique candidate is presented for explicit adoption, never silently trusted.
    return {
      state: "creation_unknown",
      candidates: [
        ...new Set([
          ...(local?.chatId ? [local.chatId] : []),
          ...(await telegram.candidates(creationMarker(setup))),
        ]),
      ],
    }
  }
  await journal.write(setup.requestId, {
    fingerprint,
    state: setup.state,
    chatId: setup.chatId,
    binding: setup.binding,
    workerId,
    pendingInvitations,
    ...counts(),
  })
  // Deal Desk releases the claim during retry and join waits, so any worker
  // may claim here. Adds and invitations are only in this host's journal:
  // run one worker per custody account (RUNBOOK, Custody Chat Provisioning).
  const claimed = await cloud.claim(setup.requestId, workerId)
  if (requestFingerprint(claimed) !== fingerprint || !claimed.claimToken)
    throw new Error(
      "Cloud claim did not preserve request or return claim token",
    )
  const claimToken = claimed.claimToken
  // Request receipts intentionally replay old responses. Never act on a claim snapshot.
  setup = { ...(await cloud.view(setup.requestId)), claimToken }
  if (requestFingerprint(setup) !== fingerprint)
    throw new Error("Cloud request changed after claim")
  if (setup.state === "ready") return setup
  if (setup.state === "cancelled") throw new Error("Setup is cancelled")
  if (
    !setup.chatId &&
    (setup.state === "creating" || setup.state === "creation_unknown")
  ) {
    if (setup.state === "creating")
      await cloud.progress(setup.requestId, claimToken, {
        state: "creation_unknown",
        binding: setup.binding,
      })
    return {
      state: "creation_unknown",
      candidates: await telegram.candidates(creationMarker(setup)),
    }
  }
  const save = async (progress: Progress): Promise<void> => {
    await journal.write(setup.requestId, {
      fingerprint,
      state: progress.state,
      chatId: progress.chatId ?? setup.chatId,
      binding: progress.binding ?? setup.binding,
      workerId,
      pendingInvitations,
      ...counts(),
    })
    setup = {
      ...(await cloud.progress(setup.requestId, claimToken, progress)),
      claimToken,
    }
    if (requestFingerprint(setup) !== fingerprint)
      throw new Error("Cloud request changed during provisioning")
  }
  if (!setup.binding && local?.binding && local.chatId)
    setup.binding = local.binding
  const binding = setup.binding ?? (await bindRequest(setup, telegram))
  if (!validBinding(binding, setup.expectedUserId))
    throw new Error("Invalid persisted account binding")
  if (!setup.binding)
    await save({
      state: setup.chatId ? "configuring" : "prepared",
      binding,
      chatId: setup.chatId,
    })
  if (!setup.chatId && local?.chatId) {
    if (!(await telegram.verifyMarker(local.chatId, creationMarker(setup))))
      throw new Error(
        "Private receipt chat does not match request marker and owner",
      )
    await save({ state: "configuring", chatId: local.chatId, binding })
  }
  let createdNow = false
  if (!setup.chatId) {
    await save({ state: "creating", binding })
    let chatId: number
    try {
      chatId = await telegram.create(setup.title, creationMarker(setup))
    } catch (error) {
      const rejected = definiteCreateRejection(error)
      await save({
        state: rejected ? "incomplete" : "creation_unknown",
        binding,
        error: safeProvisionError(error),
        // A timeout may still have created the group: only a definite
        // rejection is `incomplete`, the one state that takes a retryAt.
        ...(rejected
          ? { creationRejected: true, retryAt: provisionRetryAt(error) }
          : { retryAt: null }),
      })
      // `cause` lets the CLI map a hold refusal to exit 75.
      throw new Error(
        "Group creation failed; inspect authoritative setup state before resuming",
        { cause: error },
      )
    }
    createdNow = true
    // Persist immediately, before another network call or cloud acknowledgement.
    await journal.write(setup.requestId, {
      fingerprint,
      state: "configuring",
      chatId,
      binding,
      workerId,
      pendingInvitations,
      ...counts(),
    })
    await save({ state: "configuring", chatId, binding })
  }
  const chatId = setup.chatId!
  let step: ProvisionStep = "verify_marker"
  // People this check left waiting on their invitation, if any.
  let waitingOn: number[] = []
  try {
    if (!(await telegram.verifyMarker(chatId, creationMarker(setup))))
      throw new Error("Chat marker or custody owner verification failed")
    // A re-check of a chat waiting on people to join keeps its state, so
    // requesters see no "configuring" flicker every 15 minutes.
    if (setup.state !== "awaiting_join") {
      step = "save_configuring"
      await save({ state: "configuring", chatId, binding })
    }
    // Requester first: only required participants (the requester and team
    // admins) and the bot join in this job. Optional members go to the
    // background queue once the chat verifies.
    const optional = new Set(binding.optionalParticipants ?? [])
    const required = binding.participants.filter((id) => !optional.has(id))
    const absent: number[] = []
    // Added earlier but not yet visible, or refused for privacy, and given
    // a controlled invitation: never add again. A live invitation means
    // wait; an expired one means stop for an operator.
    const invited: number[] = []
    const expired: number[] = []
    // Added by an earlier run but absent, with no invitation yet: a link,
    // never another add.
    const addedAbsent: number[] = []
    const notVisibleInvitation = (userId: number) =>
      pendingInvitations.find(
        (i) =>
          i.userId === String(userId) &&
          WAIT_INVITATION_REASONS.has(i.reason),
      )
    if (createdNow) {
      // The worker just created this chat: only the custody account is in
      // it, so no membership read comes before the add.
      absent.push(...required, PROVISIONING_BOT_ID)
    } else {
      // A resumed job reads each user once, then adds everyone absent.
      for (const userId of [...required, PROVISIONING_BOT_ID]) {
        step = "read_member"
        const member = await telegram.member(chatId, userId)
        if (member.role !== "absent") continue
        const invitation = notVisibleInvitation(userId)
        if (!invitation && addedIds.has(userId)) addedAbsent.push(userId)
        else if (!invitation) absent.push(userId)
        else if (Date.parse(invitation.expiresAt) > Date.now())
          invited.push(userId)
        else expired.push(userId)
      }
      // A controlled invitation needs approval: approve only the people it
      // was made for. Errors other than "no request" propagate, so a flood
      // is saved with its own retry time.
      for (const list of [invited, expired])
        for (const userId of [...list]) {
          if (!telegram.approveJoin) break
          step = "approve_join"
          if (!(await telegram.approveJoin(chatId, userId))) continue
          step = "read_member"
          if ((await telegram.member(chatId, userId)).role !== "absent")
            list.splice(list.indexOf(userId), 1)
        }
      if (expired.length) {
        step = "verify_added_member"
        throw Object.assign(new Error("Member invitation expired"), {
          text: INVITATION_EXPIRED,
        })
      }
    }
    const writeJournal = (next: Partial<Counts> = {}) =>
      journal.write(setup.requestId, {
        fingerprint,
        state: "configuring",
        chatId,
        binding,
        workerId,
        pendingInvitations,
        ...counts(next),
      })
    /** Writes changed counters, then keeps them; a failed write keeps none. */
    const commit = async (next: Partial<Counts>): Promise<void> => {
      await writeJournal(next)
      if (next.botAdds !== undefined) botAdds = next.botAdds
      if (next.transientRetries !== undefined)
        transientRetries = next.transientRetries
      if (next.addedIds !== undefined) addedIds = next.addedIds
    }
    // The bot cannot use a link, so a bot that never shows would cost an
    // add on every retry: each call that may reach it counts (`sending`).
    if (absent.includes(PROVISIONING_BOT_ID) && botAdds >= MAX_BOT_ADDS) {
      step = "verify_added_member"
      throw Object.assign(new Error("Bot still not visible after re-add"), {
        text: BOT_ADDS_EXHAUSTED,
      })
    }
    // Before each add call, the people and the bot it covers are journaled
    // as possibly added: a crash or lost response after Telegram accepted
    // it can never lead to a second add. Only a known refusal of the whole
    // call (a flood, a hold, or a per-user 400) is taken back.
    const sending = async (userIds: number[]): Promise<void> => {
      // After shutdown the client is destroyed and no call leaves the
      // process, so nobody may be marked possibly added.
      if (params.signal?.aborted)
        throw new DOMException("Worker is shutting down", "AbortError")
      const people = userIds.filter((id) => id !== PROVISIONING_BOT_ID)
      await commit({
        botAdds: botAdds + (userIds.length - people.length),
        addedIds: new Set([...addedIds, ...people]),
      })
      // Shutdown during that write: the call will not be sent.
      if (params.signal?.aborted) {
        await notAdded(userIds)
        throw new DOMException("Worker is shutting down", "AbortError")
      }
    }
    /**
     * Takes back a call Telegram refused for certain. The refusal is a
     * fact, so it is kept in memory even if this write fails: the run's
     * final save journals it. The caller rethrows the Telegram error so its
     * retry time is kept.
     */
    const notAdded = async (userIds: number[]): Promise<void> => {
      if (!userIds.length) return
      const people = new Set(userIds.filter((id) => id !== PROVISIONING_BOT_ID))
      const next = {
        botAdds: botAdds - (userIds.length - people.size),
        addedIds: new Set([...addedIds].filter((id) => !people.has(id))),
      }
      await commit(next).catch(() => {
        botAdds = next.botAdds
        addedIds = next.addedIds
      })
    }
    const refusedWhole = (error: unknown): boolean =>
      isPerUserAddError(error) ||
      (!isTransientTelegramError(error) && provisionRetryAt(error) !== null)
    // Per-user refusals (privacy, USER_*/CHAT_* 400s) never stop the other
    // users: a refused batch falls back to one add per user. Flood and hold
    // errors refuse the whole batch and propagate with their retry time.
    const failures = new Map<number, { error: unknown; step: ProvisionStep }>()
    const addEach = async (userIds: number[]): Promise<void> => {
      for (const userId of userIds) {
        step = "add_member"
        await sending([userId])
        try {
          await telegram.add(chatId, userId)
        } catch (error) {
          if (refusedWhole(error)) await notAdded([userId])
          if (!isPerUserAddError(error)) throw error
          failures.set(userId, { error, step: "add_member" })
        }
      }
    }
    if (absent.length && telegram.addMany) {
      step = "add_member"
      await sending(absent)
      try {
        const missing = new Set(await telegram.addMany(chatId, absent))
        // Telegram reports privacy refusals; those people were not added.
        await notAdded(absent.filter((id) => missing.has(id)))
        for (const userId of absent)
          if (missing.has(userId))
            failures.set(userId, {
              error: { text: "USER_PRIVACY_RESTRICTED" },
              step: "add_member",
            })
      } catch (error) {
        // A refused batch added no one; the per-user fallback counts again.
        if (refusedWhole(error)) await notAdded(absent)
        if (!isPerUserAddError(error)) throw error
        await addEach(absent)
      }
    } else await addEach(absent)
    // This run got past the adds, so earlier transient add failures no
    // longer count toward the limit.
    if (transientRetries) await commit({ transientRetries: 0 })
    // With a participant listing, the final audit verifies the adds.
    const listed = createdNow && Boolean(telegram.members)
    // Telegram can accept an add before the member shows up: read again
    // once after a short wait before treating the add as incomplete.
    const notVisible: number[] = []
    // The bot cannot use a link; a resumed job reads it and adds it again
    // only if it is still absent (`MAX_BOT_ADDS`).
    let botNotVisible = false
    if (!listed) {
      const unseen = async (ids: number[]): Promise<number[]> => {
        const out: number[] = []
        for (const userId of ids) {
          step = "verify_added_member"
          if ((await telegram.member(chatId, userId)).role === "absent")
            out.push(userId)
        }
        return out
      }
      let missing = await unseen(absent.filter((id) => !failures.has(id)))
      if (missing.length) {
        await (params.sleep ?? defaultSleep)(MEMBER_RECHECK_MS)
        missing = await unseen(missing)
      }
      for (const userId of missing)
        if (userId === PROVISIONING_BOT_ID) botNotVisible = true
        else if (!telegram.inviteLink)
          failures.set(userId, {
            error: new Error("Participant invitation incomplete"),
            step: "verify_added_member",
          })
        else notVisible.push(userId)
    }
    // People an earlier run added who are still absent wait on a link,
    // whichever step that run stopped at.
    if (addedAbsent.length && !telegram.inviteLink) {
      step = "verify_added_member"
      throw new Error("Participant invitation incomplete")
    }
    notVisible.push(...addedAbsent)
    // A person refused for privacy can still join through a controlled
    // invitation, so the setup waits for them like a member not visible.
    const privacyRefused = absent.filter(
      (userId) =>
        userId !== PROVISIONING_BOT_ID &&
        Boolean(telegram.inviteLink) &&
        isPrivacyRestriction(failures.get(userId)?.error),
    )
    for (const userId of privacyRefused) failures.delete(userId)
    /** Creates one shared link for `entries` and saves it in the journal. */
    const invite = async (
      entries: { userId: number; reason: string }[],
    ): Promise<void> => {
      if (!entries.length) return
      // One shared link: the first person to need it decides its owner.
      step = "create_invitation"
      const created = await controlledInvitation(
        telegram,
        chatId,
        entries[0]!.userId,
      )
      pendingInvitations = [
        ...pendingInvitations.filter(
          (i) => !entries.some((n) => n.userId === Number(i.userId)),
        ),
        ...entries.map(({ userId, reason }) => ({
          userId: String(userId),
          ...created,
          reason,
        })),
      ]
      step = "save_invitation"
      await writeJournal()
    }
    /**
     * Throws for people (and the bot) Telegram accepted but does not show:
     * people get an `added_not_visible` invitation first, so no resume adds
     * them again; the bot stays under `MAX_BOT_ADDS`.
     */
    const notShown = async (userIds: number[]): Promise<never> => {
      const failedStep = step
      const people = userIds.filter((id) => id !== PROVISIONING_BOT_ID)
      if (people.length && !telegram.inviteLink)
        throw new Error("Participant invitation incomplete")
      await invite(
        people.map((userId) => ({
          userId,
          reason: NOT_VISIBLE_INVITATION_REASON,
        })),
      )
      step = failedStep
      if (!userIds.includes(PROVISIONING_BOT_ID)) waitingOn = people
      throw userIds.includes(PROVISIONING_BOT_ID)
        ? botNotVisibleError()
        : notVisibleError()
    }
    const needLink = [
      ...notVisible.map((userId) => ({
        userId,
        reason: NOT_VISIBLE_INVITATION_REASON,
      })),
      ...privacyRefused.map((userId) => ({
        userId,
        reason: PRIVACY_INVITATION_REASON,
      })),
    ]
    // Saved before any per-user failure is thrown, so a later resume never
    // adds an invited member again.
    await invite(needLink)
    for (const userId of absent) {
      const failure = failures.get(userId)
      if (!failure) continue
      step = failure.step
      throw failure.error
    }
    const waiting = [...notVisible, ...privacyRefused, ...invited]
    if (waiting.length) waitingOn = waiting
    if (waiting.length || botNotVisible) {
      // Only privacy invitations outstanding: the person must act.
      const privacyOnly =
        !botNotVisible &&
        waiting.every((userId) =>
          pendingInvitations.some(
            (i) =>
              i.userId === String(userId) &&
              i.reason === PRIVACY_INVITATION_REASON,
          ),
        )
      step = privacyOnly ? "add_member" : "verify_added_member"
      // A bot still owed a re-add is not an invitation wait.
      throw botNotVisible
        ? botNotVisibleError()
        : privacyOnly
          ? Object.assign(new Error("Invited member has not joined yet"), {
              text: AWAITING_JOIN,
            })
          : notVisibleError()
    }
    // Promoting a user Telegram does not show in the chat yet is the same
    // propagation delay as a read: retry later instead of failing.
    const promote = async (
      userId: number,
      rights: Record<string, boolean>,
    ): Promise<void> => {
      try {
        await telegram.promote(chatId, userId, rights)
      } catch (error) {
        const text = (error as { text?: unknown } | null)?.text
        if (text === "USER_NOT_PARTICIPANT") await notShown([userId])
        throw error
      }
    }
    for (const userId of binding.admins) {
      step = "read_admin"
      if (
        listed ||
        !hasRights(await telegram.member(chatId, userId), TEAM_ADMIN_RIGHTS)
      ) {
        step = "promote_admin"
        await promote(userId, TEAM_ADMIN_RIGHTS)
      }
    }
    step = "read_bot"
    if (
      listed ||
      !hasRights(
        await telegram.member(chatId, PROVISIONING_BOT_ID),
        BOT_BASELINE_RIGHTS,
      )
    ) {
      step = "promote_bot"
      await promote(PROVISIONING_BOT_ID, BOT_BASELINE_RIGHTS)
    }
    step = "audit_membership"
    // One participant listing when the adapter has one; else one read each.
    const roster = telegram.members ? await telegram.members(chatId) : null
    const memberOf = async (userId: number): Promise<Member> =>
      roster
        ? (roster.get(userId) ?? { role: "absent", rights: {} })
        : telegram.member(chatId, userId)
    const owner = await memberOf(setup.expectedUserId)
    const participants: (Member & { userId: number })[] = []
    for (const userId of binding.participants)
      participants.push({ userId, ...(await memberOf(userId)) })
    const bot = await memberOf(PROVISIONING_BOT_ID)
    const backgroundIds = participants
      .filter((m) => optional.has(m.userId) && m.role === "absent")
      .map((m) => m.userId)
    // A required member or the bot missing from the listing right after
    // their add is a propagation delay; a resumed job reads them again.
    // A wrong owner or wrong rights is not, and needs an operator.
    const unlisted = [
      ...participants
        .filter((m) => m.role === "absent" && !backgroundIds.includes(m.userId))
        .map((m) => m.userId),
      ...(bot.role === "absent" ? [PROVISIONING_BOT_ID] : []),
    ]
    if (owner.role === "creator" && unlisted.length) await notShown(unlisted)
    if (
      owner.role !== "creator" ||
      participants.some((member) =>
        binding.admins.includes(member.userId)
          ? !hasRights(member, TEAM_ADMIN_RIGHTS)
          : member.role !== "member" && !backgroundIds.includes(member.userId),
      ) ||
      !hasRights(bot, BOT_BASELINE_RIGHTS)
    )
      throw new Error("Final membership or rights audit failed")
    if (backgroundIds.length) {
      step = "enqueue_background"
      if (!params.background)
        throw new Error("Background member queue required")
      // `state.db` can be busy or briefly unreadable; retry, never strand.
      const queue = params.background
      await Promise.resolve()
        .then(() =>
          queue.enqueue({
            requestId: setup.requestId,
            chatId,
            userIds: backgroundIds,
          }),
        )
        .catch(() => {
          throw Object.assign(new Error("Background queue write failed"), {
            text: QUEUE_UNAVAILABLE,
          })
        })
      // Deal Desk accepts an absent optional member only with a pending
      // invitation, so every queued member shares one controlled link.
      let link = pendingInvitations.find(
        (i) =>
          i.reason === BACKGROUND_INVITATION_REASON &&
          Date.parse(i.expiresAt) > Date.now() + MIN_INVITATION_LEAD_MS,
      )
      if (!link) {
        step = "create_invitation"
        if (!telegram.inviteLink)
          throw new Error("Invitation links required for background members")
        const created = await controlledInvitation(
          telegram,
          chatId,
          backgroundIds[0]!,
        )
        link = {
          userId: String(backgroundIds[0]),
          ...created,
          reason: BACKGROUND_INVITATION_REASON,
        }
      }
      const shared = link
      pendingInvitations = backgroundIds.map((userId) => ({
        userId: String(userId),
        inviteLink: shared.inviteLink,
        expiresAt: shared.expiresAt,
        reason: BACKGROUND_INVITATION_REASON,
      }))
      step = "save_invitation"
      await journal.write(setup.requestId, {
        fingerprint,
        state: "configuring",
        chatId,
        binding,
        workerId,
        pendingInvitations,
        ...counts(),
      })
    } else pendingInvitations = []
    const verifiedAt = new Date().toISOString()
    // Link retrieval is presentation metadata, not a membership requirement.
    // Preserve readiness even when Telegram temporarily refuses these reads.
    const links = await telegram.completionLinks?.(chatId).catch(() => ({}))
    const receipt = {
      chatId: String(chatId),
      ownerId: String(setup.expectedUserId),
      participants: participants.map((member) => ({
        ...member,
        userId: String(member.userId),
      })),
      bot: { ...bot, userId: String(PROVISIONING_BOT_ID) },
      verifiedAt,
      ...(pendingInvitations.length ? { pendingInvitations } : {}),
      ...(backgroundIds.length
        ? {
            backgroundMembers: backgroundIds.map((userId) => ({
              userId: String(userId),
              state: "pending" as const,
            })),
          }
        : {}),
      ...links,
    }
    step = "save_ready"
    await save({ state: "ready", chatId, binding, receipt, retryAt: null })
    return setup
  } catch (error) {
    // Never discard known chat identity after partial setup or an ambiguous cloud commit.
    const authoritative = await cloud.view(setup.requestId)
    if (authoritative.state === "ready") return authoritative
    let diagnostic = safeProvisionDiagnostic(error, step)
    let retryAt = provisionRetryAt(error)
    // Only add calls count: their effect is uncertain and they spend the
    // custody budget. Reads, approvals, and promotions spend no add, and a
    // run cut short by worker shutdown is not Telegram's failure. The count
    // resets once a run gets past the adds.
    const transient = isTransientTelegramError(error) && !params.signal?.aborted
    if (
      step === "add_member" &&
      transient &&
      ++transientRetries >= MAX_TRANSIENT_RETRIES
    ) {
      diagnostic = `${step}:${TRANSIENT_LIMIT}`
      retryAt = null
    }
    // Other steps spend no add but keep the account busy in Deal Desk, so
    // runs that keep stopping there also end with an operator, after about
    // an hour. `stalledStep` is the furthest step a counted stall reached:
    // a transient stop beyond it starts over, one at or before it counts.
    // A flood or hold has its own retry time and leaves the count alone, and
    // so does a shutdown. `create_invitation`
    // runs at several points; its one place in `STEP_ORDER` only means a
    // stall there may count instead of resetting.
    const furthest = STEP_ORDER.indexOf(stalledStep as ProvisionStep)
    const beyond = furthest < 0 || STEP_ORDER.indexOf(step) > furthest
    if (params.signal?.aborted || isFloodOrHold(error)) {
      // No progress either way.
    } else if (transient && step !== "add_member") {
      if (beyond) stalledStep = step
      stalledRuns = beyond ? 1 : stalledRuns + 1
      if (stalledRuns >= MAX_STALLED_RUNS) {
        diagnostic = `${step}:${TRANSIENT_LIMIT}`
        retryAt = null
      }
    } else if (!isTransientTelegramError(error)) {
      // Any other stop (a link wait, a bot wait, an operator error) means
      // Telegram answered every call up to it, so the stall is over.
      stalledRuns = 0
      stalledStep = undefined
    }
    const awaiting = waitingOn.length
      ? pendingInvitations.filter((i) =>
          waitingOn.some((userId) => String(userId) === i.userId),
        )
      : []
    if (
      awaiting.length &&
      retryAt &&
      waitingOnInvitedMember({ error: diagnostic })
    ) {
      // Only invited people can finish this chat. Deal Desk releases the
      // claim, so other setups run until the next check is due.
      await save({
        state: "awaiting_join",
        chatId,
        binding,
        error: diagnostic,
        nextCheckAt: retryAt,
        receipt: { pendingInvitations: awaiting },
      })
      throw new Error(
        "Setup waiting on invited people to join; it resumes at its next check",
        { cause: error },
      )
    }
    await save({
      state: "incomplete",
      chatId,
      binding,
      error: diagnostic,
      retryAt,
    })
    throw new Error(
      "Setup incomplete; inspect status and resume the same request",
      { cause: error },
    )
  }
}
