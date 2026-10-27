import { createHash } from "node:crypto"

export const PROVISIONING_BOT_ID = 8983431101
export type Peer = number | string
export interface ProvisionRequest {
  requestId: string
  custodyProfile: string
  expectedUserId: number
  title: string
  participants: Peer[]
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
}
export interface Setup extends ProvisionRequest {
  state: ProvisionState
  chatId?: number
  binding?: Binding
  claimToken?: string
  receipt?: unknown
}
export interface Member {
  role: "creator" | "admin" | "member" | "absent"
  rights: Record<string, boolean>
}
export interface TelegramProvisioner {
  self(): Promise<number>
  resolve(peer: Peer): Promise<{ id: number; isBot: boolean }>
  create(title: string, marker: string): Promise<number>
  member(chatId: number, userId: number): Promise<Member>
  add(chatId: number, userId: number): Promise<void>
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
  inviteUsers: true,
  pinMessages: true,
}
export const TEAM_ADMIN_RIGHTS = {
  ...rightsOff,
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
  const binding = { participants, admins }
  if (!validBinding(binding, request.expectedUserId))
    throw new Error("Invalid account binding")
  return binding
}

/** Only explicit server rejection proves creation did not happen. Transport failures remain unknown. */
export function definiteCreateRejection(error: unknown): boolean {
  const e = error as { text?: string; code?: number }
  return (
    [400, 401, 403, 420].includes(e?.code ?? 0) &&
    typeof e.text === "string" &&
    /^(?:FLOOD_WAIT|CHANNELS_TOO_MUCH|USER_RESTRICTED|CHAT_TITLE_EMPTY|CHAT_ABOUT_TOO_LONG|AUTH_KEY_UNREGISTERED|SESSION_REVOKED)(?:_%d|_\d+)?$/.test(
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
  return /^[A-Z][A-Z0-9_]+$/.test(normalized)
    ? normalized
    : "provisioning_step_failed"
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
  if (setup.state === "ready") return setup
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
      await save({
        state: definiteCreateRejection(error)
          ? "incomplete"
          : "creation_unknown",
        binding,
        error: safeProvisionError(error),
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
    })
    await save({ state: "configuring", chatId, binding })
  }
  const chatId = setup.chatId!
  try {
    if (!(await telegram.verifyMarker(chatId, creationMarker(setup))))
      throw new Error("Chat marker or custody owner verification failed")
    await save({ state: "configuring", chatId, binding })
    for (const userId of [...binding.participants, PROVISIONING_BOT_ID]) {
      const member = await telegram.member(chatId, userId)
      if (member.role === "absent") await telegram.add(chatId, userId)
      if ((await telegram.member(chatId, userId)).role === "absent")
        throw new Error("Participant invitation incomplete")
    }
    for (const userId of binding.admins) {
      if (!hasRights(await telegram.member(chatId, userId), TEAM_ADMIN_RIGHTS))
        await telegram.promote(chatId, userId, TEAM_ADMIN_RIGHTS)
    }
    if (
      !hasRights(
        await telegram.member(chatId, PROVISIONING_BOT_ID),
        BOT_BASELINE_RIGHTS,
      )
    )
      await telegram.promote(chatId, PROVISIONING_BOT_ID, BOT_BASELINE_RIGHTS)
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
          : member.role !== "member",
      ) ||
      !hasRights(bot, BOT_BASELINE_RIGHTS)
    )
      throw new Error("Final membership or rights audit failed")
    const receipt = {
      chatId: String(chatId),
      ownerId: String(setup.expectedUserId),
      participants: participants.map((member) => ({
        ...member,
        userId: String(member.userId),
      })),
      bot: { ...bot, userId: String(PROVISIONING_BOT_ID) },
      verifiedAt: new Date().toISOString(),
    }
    await save({ state: "ready", chatId, binding, receipt })
    return setup
  } catch (error) {
    // Never discard known chat identity after partial setup or an ambiguous cloud commit.
    const authoritative = await cloud.view(setup.requestId)
    if (authoritative.state === "ready") return authoritative
    await save({
      state: "incomplete",
      chatId,
      binding,
      error: safeProvisionError(error),
    })
    throw new Error(
      "Setup incomplete; inspect status and resume the same request",
    )
  }
}
