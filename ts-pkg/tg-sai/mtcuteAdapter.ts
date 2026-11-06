import { networkMiddlewares, TelegramClient } from "@mtcute/bun"
import type { FloodWaiterOptions, RpcCallMiddleware } from "@mtcute/bun"
import { createTelegramCryptoProvider } from "./telegramRuntime"
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
import type { ProfileStoreEnv } from "./profiles"
import { stateDbPath } from "./state/db"
import { createFloodGuard } from "./state/guard"
import type { HoldAccountProvider, HoldStoreSource } from "./state/guard"
import { HoldStore } from "./state/holds"
import { createTelemetryMiddlewares } from "./telemetry/middleware"
import type { ClientTelemetry } from "./telemetry/middleware"

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
  /**
   * Flood-wait settings for mtcute's flood waiter, the only flood retry
   * layer. Ignored when `telemetry.floodWaiter` is set.
   */
  floodWaiter?: FloodWaiterOptions
  /**
   * Telegram user ID the session acts as; keys the client's saved flood
   * holds. Falls back to the telemetry context's `accountId`. Without
   * either, the flood guard passes every call through.
   */
  accountId?: number
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

/** mtcute's `floodWaiter` default `maxWait`. */
export const MTCUTE_DEFAULT_FLOOD_MAX_WAIT_MS = 10_000

const holdStores = new Map<string, HoldStore>()

/**
 * The process's `HoldStore` for `state.db` under `env`'s config dir, opened
 * on first use and then shared by every client. Never throws: a store that
 * cannot open reports `unavailable`, so the guard fails closed.
 */
export function processHoldStore(
  env: ProfileStoreEnv = process.env,
): HoldStore {
  let path: string
  try {
    path = stateDbPath(env)
  } catch (error) {
    return HoldStore.unavailable(
      error instanceof Error ? error.message : String(error),
    )
  }
  let store = holdStores.get(path)
  if (!store) {
    store = HoldStore.open(path)
    holdStores.set(path, store)
  }
  return store
}

/** Closes every store `processHoldStore` opened; the next use reopens. */
export function closeProcessHoldStores(): void {
  for (const store of holdStores.values()) store.close()
  holdStores.clear()
}

/** Options for `clientMiddlewares` beyond the flood waiter and telemetry. */
export interface ClientMiddlewareOptions {
  /** Account for the flood guard; else the telemetry context's. */
  accountId?: number
  /** Hold store; default `processHoldStore()`, opened on first use. */
  holdStore?: HoldStoreSource
}

/**
 * Network middlewares for a client. Passing middlewares replaces mtcute's
 * defaults, so both branches include `networkMiddlewares.basic()` (flood
 * waiter and 5xx retry) with the same options, wrapped by the flood guard:
 *
 * - without telemetry: `[check, ...basic({ floodWaiter }), record]`
 * - with telemetry: `[outer, check, ...basic(), inner, record]`
 *
 * The guard saves waits longer than the flood waiter's `maxWait` (and all
 * invite/create waits and `PEER_FLOOD`) and refuses held calls in every
 * process. It does nothing for a client with no account ID.
 */
export function clientMiddlewares(
  floodWaiter: FloodWaiterOptions | undefined,
  telemetry: ClientTelemetry | undefined,
  options: ClientMiddlewareOptions = {},
): RpcCallMiddleware[] {
  const waiter = telemetry?.floodWaiter ?? floodWaiter
  const accountProvider: HoldAccountProvider = () =>
    options.accountId ?? telemetry?.context().accountId
  const guardOptions = {
    sleepThresholdMs: waiter?.maxWait ?? MTCUTE_DEFAULT_FLOOD_MAX_WAIT_MS,
  }
  const store: HoldStoreSource = options.holdStore ?? (() => processHoldStore())
  if (telemetry) {
    return createTelemetryMiddlewares(
      telemetry.sink,
      telemetry.context,
      waiter,
      (requestIdOf) =>
        createFloodGuard(store, accountProvider, {
          ...guardOptions,
          requestIdOf,
        }),
    )
  }
  const guard = createFloodGuard(store, accountProvider, guardOptions)
  return [
    guard.check,
    ...networkMiddlewares.basic(waiter ? { floodWaiter: waiter } : undefined),
    guard.record,
  ]
}

/**
 * Shared constructor used by offline readiness and authenticated operations.
 * Every client gets the flood guard (see `clientMiddlewares`). With
 * `telemetry`, every RPC call, wire attempt, and flood wait is recorded.
 */
export function createMtcuteRuntimeClient(
  config: Omit<MtcuteClientConfig, "session">,
  telemetry?: ClientTelemetry,
): TelegramClient {
  const middlewares = clientMiddlewares(config.floodWaiter, telemetry, {
    accountId: config.accountId,
  })
  return new TelegramClient({
    crypto: createTelegramCryptoProvider(),
    apiId: config.apiId,
    apiHash: config.apiHash,
    storage: config.storage ?? "sai-telegram-admin-check.session",
    network: { middlewares },
  })
}

/**
 * Creates and authenticates a mtcute client from a Telethon session string.
 */
export async function createMtcuteClient(
  config: MtcuteClientConfig,
  telemetry?: ClientTelemetry,
): Promise<TelegramClient> {
  const client = createMtcuteRuntimeClient(config, telemetry)

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
