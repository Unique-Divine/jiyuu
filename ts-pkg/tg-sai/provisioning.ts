import { createHash } from "node:crypto"

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
  /** Read existing links only. Failure must not invalidate verified setup. */
  completionLinks?(chatId: number): Promise<ProvisioningLinks>
  promote(
    chatId: number,
    userId: number,
    rights: Record<string, boolean>,
  ): Promise<void>
  candidates(marker: string): Promise<number[]>
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
}
export interface PendingInvitation {
  userId: string
  inviteLink: string
  expiresAt: string
  reason: string
}
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
  // Telegram: a hold (420) or an unreadable hold store (500).
  if (
    (e.code === 420 && /^TG_SAI_HOLD_\d+$/.test(e.text)) ||
    (e.code === 500 && e.text === HOLD_STATE_UNAVAILABLE)
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
  return /^(?:FLOOD(?:_PREMIUM)?_WAIT_(?:\d{1,10}|UNKNOWN)|PEER_FLOOD|TG_SAI_HOLD_\d{1,10}|TG_SAI_HOLD_STATE_UNAVAILABLE|USER_PRIVACY_RESTRICTED|USER_NOT_MUTUAL_CONTACTS?|USER_NOT_PARTICIPANT|USER_CHANNELS_TOO_MUCH|USER_BOT|USER_ID_INVALID|PEER_ID_INVALID|CHAT_ID_INVALID|CHANNEL_INVALID|CHANNEL_PRIVATE|CHAT_ADMIN_REQUIRED|CHAT_WRITE_FORBIDDEN|USER_ADMIN_INVALID|USER_CREATOR|ADMINS_TOO_MUCH|BOTS_TOO_MUCH|BOT_GROUPS_BLOCKED|RIGHT_FORBIDDEN|USER_RESTRICTED|USER_DEACTIVATED|INPUT_USER_DEACTIVATED|CHANNELS_TOO_MUCH|USERS_TOO_MUCH|AUTH_KEY_UNREGISTERED|SESSION_REVOKED|INVITE_HASH_EXPIRED|INVITE_HASH_INVALID|INVITE_REQUEST_SENT)$/.test(
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
  | "create_invitation"
  | "save_invitation"
  | "read_admin"
  | "promote_admin"
  | "read_bot"
  | "promote_bot"
  | "audit_membership"
  | "save_ready"

/** Fixed operation names add context without copying Telegram response bodies. */
export function safeProvisionDiagnostic(
  error: unknown,
  step: ProvisionStep,
): string {
  return `${step}:${safeProvisionError(error)}`
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
/** Telegram gives no duration for `PEER_FLOOD`; holds use 24 h. */
export const PEER_FLOOD_RETRY_MS = 24 * 60 * 60 * 1000
const MAX_RETRY_MS = 31_536_000 * 1000

/**
 * When a flood or hold error allows an unattended retry: `FLOOD_WAIT_N`
 * (also mtcute's `FLOOD_WAIT_%d` with `seconds`), `FLOOD_PREMIUM_WAIT_N`,
 * `PEER_FLOOD` (24 h), `TG_SAI_HOLD_N` (the hold's `untilMs` when present),
 * and `TG_SAI_HOLD_STATE_UNAVAILABLE` (5 min). `null` for anything else.
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
  const at = (delayMs: number): string | null =>
    Number.isSafeInteger(delayMs) && delayMs > 0 && delayMs <= MAX_RETRY_MS
      ? new Date(now + delayMs).toISOString()
      : null
  if (code === "PEER_FLOOD") return at(PEER_FLOOD_RETRY_MS)
  if (code === HOLD_STATE_UNAVAILABLE) return at(HOLD_STATE_UNAVAILABLE_RETRY_MS)
  const hold = /^TG_SAI_HOLD_(\d+)$/.exec(code)
  if (hold) {
    if (Number.isSafeInteger(e.untilMs) && e.untilMs! > now)
      return at(e.untilMs! - now)
    return at(Number(hold[1]) * 1000)
  }
  const match = /^FLOOD(?:_PREMIUM)?_WAIT_(\d+|%d)$/.exec(code)
  const seconds = match ? Number(match[1] === "%d" ? e.seconds : match[1]) : NaN
  return Number.isSafeInteger(seconds) ? at(seconds * 1000) : null
}

/** The cloud is authoritative. The journal only recovers a result not yet acknowledged by cloud. */
export async function provisionSetup(params: {
  setup: Setup
  telegram: TelegramProvisioner
  cloud: ProvisionCloud
  journal: ProvisionJournal
  workerId: string
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
  let pendingInvitations = local?.pendingInvitations ?? []
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
  })
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
        ...(rejected ? { creationRejected: true } : {}),
        retryAt: provisionRetryAt(error),
      })
      throw new Error(
        "Group creation failed; inspect authoritative setup state before resuming",
      )
    }
    // Persist immediately, before another network call or cloud acknowledgement.
    await journal.write(setup.requestId, {
      fingerprint,
      state: "configuring",
      chatId,
      binding,
      workerId,
      pendingInvitations,
    })
    await save({ state: "configuring", chatId, binding })
  }
  const chatId = setup.chatId!
  let step: ProvisionStep = "verify_marker"
  try {
    if (!(await telegram.verifyMarker(chatId, creationMarker(setup))))
      throw new Error("Chat marker or custody owner verification failed")
    step = "save_configuring"
    await save({ state: "configuring", chatId, binding })
    // One membership read per user, then one batched add for everyone absent.
    const absent: number[] = []
    for (const userId of [...binding.participants, PROVISIONING_BOT_ID]) {
      step = "read_member"
      const member = await telegram.member(chatId, userId)
      if (member.role !== "absent") {
        pendingInvitations = pendingInvitations.filter(
          (invite) => invite.userId !== String(userId),
        )
        continue
      }
      absent.push(userId)
    }
    // Per-user privacy refusals never stop the other users; flood and hold
    // errors refuse the whole batch and propagate with their retry time.
    const failures = new Map<number, { error: unknown; step: ProvisionStep }>()
    const addEach = async (userIds: number[]): Promise<void> => {
      for (const userId of userIds) {
        try {
          step = "add_member"
          await telegram.add(chatId, userId)
        } catch (error) {
          if (!isPrivacyRestriction(error)) throw error
          failures.set(userId, { error, step: "add_member" })
        }
      }
    }
    if (absent.length && telegram.addMany) {
      step = "add_member"
      try {
        const missing = new Set(await telegram.addMany(chatId, absent))
        for (const userId of absent)
          if (missing.has(userId))
            failures.set(userId, {
              error: { text: "USER_PRIVACY_RESTRICTED" },
              step: "add_member",
            })
      } catch (error) {
        if (!isPrivacyRestriction(error)) throw error
        await addEach(absent)
      }
    } else await addEach(absent)
    for (const userId of absent) {
      if (failures.has(userId)) continue
      step = "verify_added_member"
      if ((await telegram.member(chatId, userId)).role === "absent")
        failures.set(userId, {
          error: new Error("Participant invitation incomplete"),
          step: "verify_added_member",
        })
    }
    let fatal: { error: unknown; step: ProvisionStep } | undefined
    for (const userId of absent) {
      const failure = failures.get(userId)
      if (!failure) continue
      if (
        !binding.optionalParticipants?.includes(userId) ||
        !isPrivacyRestriction(failure.error) ||
        !telegram.inviteLink
      ) {
        fatal ??= failure
        continue
      }
      let invite = pendingInvitations.find(
        (i) =>
          i.userId === String(userId) && Date.parse(i.expiresAt) > Date.now(),
      )
      if (!invite) {
        step = "create_invitation"
        const link = await telegram.inviteLink(chatId, userId)
        if (
          !/^https:\/\/t\.me\/(?:\+|joinchat\/)[A-Za-z0-9_-]+$/.test(
            link.inviteLink,
          ) ||
          !Number.isFinite(Date.parse(link.expiresAt)) ||
          Date.parse(link.expiresAt) <= Date.now()
        )
          throw new Error("Invalid controlled invitation link")
        invite = {
          userId: String(userId),
          ...link,
          reason: "privacy_restricted",
        }
        pendingInvitations = [
          ...pendingInvitations.filter((i) => i.userId !== invite!.userId),
          invite,
        ]
        step = "save_invitation"
        await save({ state: "configuring", chatId, binding })
      }
    }
    if (fatal) {
      step = fatal.step
      throw fatal.error
    }
    for (const userId of binding.admins) {
      step = "read_admin"
      if (
        !hasRights(await telegram.member(chatId, userId), TEAM_ADMIN_RIGHTS)
      ) {
        step = "promote_admin"
        await telegram.promote(chatId, userId, TEAM_ADMIN_RIGHTS)
      }
    }
    step = "read_bot"
    if (
      !hasRights(
        await telegram.member(chatId, PROVISIONING_BOT_ID),
        BOT_BASELINE_RIGHTS,
      )
    ) {
      step = "promote_bot"
      await telegram.promote(chatId, PROVISIONING_BOT_ID, BOT_BASELINE_RIGHTS)
    }
    step = "audit_membership"
    const owner = await telegram.member(chatId, setup.expectedUserId)
    const participants = await Promise.all(
      binding.participants.map(async (userId) => ({
        userId,
        ...(await telegram.member(chatId, userId)),
      })),
    )
    const bot = await telegram.member(chatId, PROVISIONING_BOT_ID)
    if (
      owner.role !== "creator" ||
      participants.some((member) =>
        binding.admins.includes(member.userId)
          ? !hasRights(member, TEAM_ADMIN_RIGHTS)
          : member.role !== "member" &&
            !(
              binding.optionalParticipants?.includes(member.userId) &&
              member.role === "absent" &&
              pendingInvitations.some(
                (invite) =>
                  invite.userId === String(member.userId) &&
                  Date.parse(invite.expiresAt) > Date.now(),
              )
            ),
      ) ||
      !hasRights(bot, BOT_BASELINE_RIGHTS)
    )
      throw new Error("Final membership or rights audit failed")
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
      ...links,
    }
    step = "save_ready"
    await save({ state: "ready", chatId, binding, receipt, retryAt: null })
    return setup
  } catch (error) {
    // Never discard known chat identity after partial setup or an ambiguous cloud commit.
    const authoritative = await cloud.view(setup.requestId)
    if (authoritative.state === "ready") return authoritative
    await save({
      state: "incomplete",
      chatId,
      binding,
      error: safeProvisionDiagnostic(error, step),
      retryAt: provisionRetryAt(error),
    })
    throw new Error(
      "Setup incomplete; inspect status and resume the same request",
    )
  }
}
