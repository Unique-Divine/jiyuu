import { TelegramClient } from "@mtcute/bun"
import {
  addChatMembers,
  editAdminRights,
  getChatMember,
  iterDialogs,
  transferChatOwnership,
} from "@mtcute/bun/methods.js"
import { convertFromTelethonSession } from "@mtcute/convert"
import type { ChatAuditInput, ChatMemberData, UserData } from "./adminAudit"
import type { SaiChatRecord } from "./ops"
import { isSaiChatTitle } from "./ops"

/**
 * Connection settings used to create a mtcute Telegram client.
 */
export interface MtcuteClientConfig {
  /** Telegram API ID from my.telegram.org. */
  apiId: number
  /** Telegram API hash from my.telegram.org. */
  apiHash: string
  /** Telethon-compatible session string for the acting user account. */
  session: string
  /** Optional local mtcute storage path. */
  storage?: string
}

/**
 * Parameters needed to audit one target user or bot in one chat.
 */
export interface ChatAuditFetchParams {
  /** Telegram chat ID or resolvable chat peer. */
  chatId: number | string
  /** Target user or bot ID to inspect with `getChatMember`. */
  targetUserId: number | string
}

/**
 * Parameters for actions that add or otherwise address one target peer.
 */
export interface TargetWriteParams {
  /** Telegram chat ID or resolvable chat peer. */
  chatId: number | string
  /** Target user or bot peer used for write operations. */
  targetPeer: number | string
}

/**
 * Parameters for promoting a target peer to admin.
 */
export interface PromoteTargetParams extends TargetWriteParams {
  /** mtcute admin-rights object passed to `editAdminRights`. */
  rights: Record<string, boolean>
  /** Optional admin rank/title. Telegram limits this value. */
  rank?: string
}

/**
 * Parameters for transferring Telegram chat ownership.
 */
export interface TransferOwnershipParams extends TargetWriteParams {
  /** Current owner's Telegram 2FA password. Do not persist this value. */
  password: string
}

type MtcuteUser = Awaited<ReturnType<TelegramClient["getMe"]>>
type MtcuteChatMember = Awaited<ReturnType<typeof getChatMember>>

/**
 * Options for discovering Sai-related chats from the active Telegram session.
 */
export interface FindSaiChatsParams {
  /** Maximum number of dialogs to inspect. */
  limit: number
  /** Archive scope to pass through to mtcute dialog iteration. */
  archived?: "include" | "exclude" | "only"
}

/**
 * Returns true when mtcute indicates the target peer is absent or unknown.
 */
function isPeerNotFound(error: unknown): boolean {
  if (!(error instanceof Error)) {
    return false
  }
  return (
    error.name === "MtPeerNotFoundError" ||
    error.message.includes("is not found in local cache")
  )
}

/**
 * Creates and authenticates a mtcute client from a Telethon session string.
 */
export async function createMtcuteClient(
  config: MtcuteClientConfig,
): Promise<TelegramClient> {
  const client = new TelegramClient({
    apiId: config.apiId,
    apiHash: config.apiHash,
    storage: config.storage ?? "sai-telegram-admin-check.session",
  })

  await client.importSession(convertFromTelethonSession(config.session), true)
  return client
}

/**
 * Converts a mtcute user object into the small fixture-friendly `UserData`
 * shape used by pure audit planning.
 */
export function normalizeUser(user: MtcuteUser): UserData {
  return {
    id: user.id,
    username: user.username ?? null,
    displayName: user.displayName,
    isBot: user.isBot,
    rawType: user.raw?._ ?? "unknown",
  }
}

/**
 * Converts a mtcute chat member object into the normalized `ChatMemberData`
 * shape used by pure audit planning.
 */
export function normalizeChatMember(
  member: MtcuteChatMember,
): ChatMemberData | null {
  if (!member) {
    return null
  }

  return {
    user: normalizeUser(member.user),
    status: member.status,
    title: member.title ?? null,
    invitedBy: member.invitedBy ? normalizeUser(member.invitedBy) : null,
    promotedBy: member.promotedBy ? normalizeUser(member.promotedBy) : null,
    restrictedBy: member.restrictedBy
      ? normalizeUser(member.restrictedBy)
      : null,
    restrictions: member.restrictions ?? null,
    isMember: member.isMember,
    permissions: member.permissions ? { ...member.permissions } : null,
    rawType: member.raw?._ ?? "unknown",
  }
}

/**
 * Looks up the target chat member and returns `null` when Telegram reports that
 * the target peer is not present in the chat.
 */
async function getTargetChatMember(
  client: TelegramClient,
  params: ChatAuditFetchParams,
): Promise<MtcuteChatMember | null> {
  try {
    return await getChatMember(client, {
      chatId: params.chatId,
      userId: params.targetUserId,
    })
  } catch (error) {
    if (isPeerNotFound(error)) {
      return null
    }
    throw error
  }
}

/**
 * Fetches all live Telegram state needed by `planAdminBootstrap`.
 *
 * The result contains the active session user, the actor's membership in the
 * chat, and the target user or bot membership in the same chat.
 */
export async function fetchChatAuditInput(
  client: TelegramClient,
  params: ChatAuditFetchParams,
): Promise<ChatAuditInput> {
  const self = await client.getMe()
  const actor = await getChatMember(client, {
    chatId: params.chatId,
    userId: "self",
  })
  const target = await getTargetChatMember(client, params)

  return {
    chatId: params.chatId,
    targetUserId: params.targetUserId,
    self: normalizeUser(self),
    actor: normalizeChatMember(actor),
    target: normalizeChatMember(target),
  }
}

/**
 * Lists visible dialogs for the active session and returns Sai-titled chats as
 * JSONL-ready records.
 */
export async function findSaiChats(
  client: TelegramClient,
  params: FindSaiChatsParams,
): Promise<SaiChatRecord[]> {
  const records: SaiChatRecord[] = []
  const archived = params.archived === "include" ? "keep" : params.archived
  for await (const dialog of iterDialogs(client, {
    limit: params.limit,
    archived: archived ?? "exclude",
  })) {
    const peer = dialog.peer
    const title = "title" in peer ? peer.title : peer.displayName
    if (!title || !isSaiChatTitle(title)) {
      continue
    }

    records.push({
      chatId: peer.id,
      title,
      type: "chatType" in peer ? peer.chatType : "user",
      username: "username" in peer ? (peer.username ?? null) : null,
    })
  }

  return records
}

/**
 * Adds a user or bot peer to a Telegram chat.
 */
export async function addTargetToChat(
  client: TelegramClient,
  params: TargetWriteParams,
): Promise<void> {
  await addChatMembers(client, params.chatId, params.targetPeer, {})
}

/**
 * Promotes a user or bot peer using the supplied admin-rights profile.
 */
export async function promoteTargetAdmin(
  client: TelegramClient,
  params: PromoteTargetParams,
): Promise<void> {
  await editAdminRights(client, {
    chatId: params.chatId,
    userId: params.targetPeer,
    rights: params.rights,
    rank: params.rank ?? "",
  })
}

/**
 * Transfers chat ownership to a target user.
 *
 * Telegram requires the current owner's user session and 2FA password. Bots
 * cannot receive ownership.
 */
export async function transferTargetOwnership(
  client: TelegramClient,
  params: TransferOwnershipParams,
): Promise<void> {
  await transferChatOwnership(client, {
    chatId: params.chatId,
    userId: params.targetPeer,
    password: params.password,
  })
}
