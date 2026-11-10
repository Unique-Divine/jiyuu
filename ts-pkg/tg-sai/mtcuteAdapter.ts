import { createHash } from "node:crypto"
import { join } from "node:path"
import { networkMiddlewares, TelegramClient } from "@mtcute/bun"
import type {
  FloodWaiterOptions,
  RpcCallMiddleware,
  RpcCallOptions,
} from "@mtcute/bun"
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
import { assertPresentMember } from "./adminAudit"
import type { SaiChatRecord } from "./ops"
import { isSaiChatTitle } from "./ops"
import type { ProfileStoreEnv } from "./profiles"
import { configDir } from "./profiles"
import { prepareStateFile, stateDbPath } from "./state/db"
import { createFloodGuard } from "./state/guard"
import type {
  FloodGuard,
  HoldAccountProvider,
  HoldStoreSource,
} from "./state/guard"
import { HoldStore, ReopeningHoldStore } from "./state/holds"
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
   * Verified Telegram user ID the session acts as (checked against
   * `getMe` by the caller); keys the client's saved flood holds. Unset: the
   * guard keys holds by the account the session's own `getMe` reports.
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

const holdStores = new Map<string, ReopeningHoldStore>()

/**
 * The process's `HoldStore` for `state.db` under `env`'s config dir, shared
 * by every client. Never throws: a store that cannot open reports
 * `unavailable`, so the guard fails closed, and a later call tries the
 * open again at most every `HOLD_STORE_REOPEN_MS`.
 */
export function processHoldStore(
  env: ProfileStoreEnv = process.env,
  nowMs: number = Date.now(),
): HoldStore {
  let path: string
  try {
    path = stateDbPath(env)
  } catch (error) {
    return HoldStore.unavailable(
      error instanceof Error ? error.message : String(error),
    )
  }
  let handle = holdStores.get(path)
  if (!handle) {
    handle = new ReopeningHoldStore(path)
    holdStores.set(path, handle)
  }
  return handle.get(nowMs)
}

/** Closes every store `processHoldStore` opened; the next use reopens. */
export function closeProcessHoldStores(): void {
  for (const handle of holdStores.values()) handle.close()
  holdStores.clear()
}

/**
 * Telegram user a client's session acts as, learned from the session's own
 * `getMe` (a `users.getUsers` call for `inputUserSelf`). mtcute's stored
 * self is not used: the shared storage file can carry another session's.
 */
export interface SessionAccount {
  id: number | null
}

/** Options for `clientMiddlewares` beyond the flood waiter and telemetry. */
export interface ClientMiddlewareOptions {
  /**
   * Verified account for the flood guard (e.g. provisioning's
   * `expectedUserId`, checked against `getMe`). Unset: the guard keys by
   * `sessionAccount`.
   */
  accountId?: number
  /** Filled from the session's `getMe` results; default a fresh cell. */
  sessionAccount?: SessionAccount
  /** Runs `getMe` so `sessionAccount` is known before invite/create. */
  resolveSessionAccount?: () => Promise<unknown>
  /** Hold store; default `processHoldStore()`, opened on first use. */
  holdStore?: HoldStoreSource
}

interface SelfRequest {
  _?: unknown
  id?: unknown
}

/** Whether `request` is `getMe`'s `users.getUsers([inputUserSelf])`. */
export function isGetMeRequest(request: unknown): boolean {
  const value = request as SelfRequest | null
  return (
    value?._ === "users.getUsers" &&
    Array.isArray(value.id) &&
    value.id.length === 1 &&
    (value.id[0] as { _?: unknown } | null)?._ === "inputUserSelf"
  )
}

/** Innermost: saves the user ID from each `getMe` result in `account`. */
function captureSessionAccount(account: SessionAccount): RpcCallMiddleware {
  return async (ctx, next) => {
    const result = await next(ctx)
    try {
      if (isGetMeRequest(ctx.request) && Array.isArray(result)) {
        const user = result[0] as { _?: unknown; id?: unknown } | undefined
        if (
          user?._ === "user" &&
          typeof user.id === "number" &&
          Number.isSafeInteger(user.id) &&
          user.id > 0
        ) {
          account.id = user.id
        }
      }
    } catch {
      // Capturing must never change the call's result.
    }
    return result
  }
}

/** `outer` runs first and hands `inner` the rest of the chain. */
function chainMiddleware(
  outer: RpcCallMiddleware,
  inner: RpcCallMiddleware,
): RpcCallMiddleware {
  return (ctx, next) => outer(ctx, (c) => inner(c, next))
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
 * process. It keys holds by `accountId` when given, else by the account
 * the session's own `getMe` reports. Until that is known, invite/create
 * calls first run `resolveSessionAccount` and are refused with
 * `HoldAccountUnknownError` if it fails; other calls pass unguarded.
 */
export function clientMiddlewares(
  floodWaiter: FloodWaiterOptions | undefined,
  telemetry: ClientTelemetry | undefined,
  options: ClientMiddlewareOptions = {},
): RpcCallMiddleware[] {
  const waiter = telemetry?.floodWaiter ?? floodWaiter
  const session = options.sessionAccount ?? { id: null }
  const verified = options.accountId
  const accountProvider: HoldAccountProvider = () => verified ?? session.id
  const guardOptions = {
    sleepThresholdMs: waiter?.maxWait ?? MTCUTE_DEFAULT_FLOOD_MAX_WAIT_MS,
    ...(verified === undefined
      ? {
          requireAccount: true,
          ...(options.resolveSessionAccount
            ? { resolveAccount: options.resolveSessionAccount }
            : {}),
        }
      : {}),
  }
  const store: HoldStoreSource = options.holdStore ?? (() => processHoldStore())
  const capture = captureSessionAccount(session)
  const withCapture = (guard: FloodGuard): FloodGuard => ({
    check: guard.check,
    record: chainMiddleware(guard.record, capture),
  })
  if (telemetry) {
    return createTelemetryMiddlewares(
      telemetry.sink,
      telemetry.context,
      waiter,
      (requestIdOf) =>
        withCapture(
          createFloodGuard(store, accountProvider, {
            ...guardOptions,
            requestIdOf,
          }),
        ),
    )
  }
  const guard = withCapture(
    createFloodGuard(store, accountProvider, guardOptions),
  )
  return [
    guard.check,
    ...networkMiddlewares.basic(waiter ? { floodWaiter: waiter } : undefined),
    guard.record,
  ]
}

const sessionAccounts = new WeakMap<TelegramClient, SessionAccount>()

/**
 * The account a client's session acts as, once its `getMe` has run; `null`
 * before that, and for clients not made by `createMtcuteRuntimeClient`.
 */
export function sessionAccountId(client: TelegramClient): number | null {
  return sessionAccounts.get(client)?.id ?? null
}

/** Directory under the tg-sai tool directory for default session caches. */
export const SESSION_CACHE_DIR = "sessions"

/**
 * Default mtcute storage path for one Telethon session string:
 * `<tool dir>/sessions/<hash>.session`, where the tool directory is
 * `~/.tg-sai` or `TG_SAI_DIR`. Keying by a hash of the session gives each
 * account its own cache, so concurrent runs as different accounts never
 * share auth keys or peer access hashes, and no run writes into the working
 * directory.
 */
export function defaultSessionStoragePath(
  session: string,
  env: ProfileStoreEnv = process.env,
): string {
  const hash = createHash("sha256").update(session).digest("hex")
  const file = `${hash.slice(0, 32)}.session`
  return join(configDir(env), SESSION_CACHE_DIR, file)
}

/**
 * Resolves the storage for an authenticated client: an explicit `storage`
 * wins; otherwise the per-session default file, created 0600 in 0700
 * directories before mtcute opens it.
 */
export function resolveSessionStorage(
  storage: string | undefined,
  session: string,
  env: ProfileStoreEnv = process.env,
): string {
  if (storage) return storage
  const path = defaultSessionStoragePath(session, env)
  prepareStateFile(path)
  return path
}

/**
 * Shared constructor used by offline readiness and authenticated operations.
 * Every client gets the flood guard (see `clientMiddlewares`). With
 * `telemetry`, every RPC call, wire attempt, and flood wait is recorded.
 * Without `storage`, the client keeps its state in memory.
 */
export function createMtcuteRuntimeClient(
  config: Omit<MtcuteClientConfig, "session">,
  telemetry?: ClientTelemetry,
): TelegramClient {
  const sessionAccount: SessionAccount = { id: null }
  let client: TelegramClient | null = null
  const middlewares = clientMiddlewares(config.floodWaiter, telemetry, {
    accountId: config.accountId,
    sessionAccount,
    resolveSessionAccount: async () => {
      if (!client) throw new Error("client not constructed")
      return client.getMe()
    },
  })
  client = new TelegramClient({
    crypto: createTelegramCryptoProvider(),
    apiId: config.apiId,
    apiHash: config.apiHash,
    storage: config.storage ?? ":memory:",
    network: { middlewares },
  })
  sessionAccounts.set(client, sessionAccount)
  return client
}

/**
 * Creates and authenticates a mtcute client from a Telethon session string.
 */
export async function createMtcuteClient(
  config: MtcuteClientConfig,
  telemetry?: ClientTelemetry,
): Promise<TelegramClient> {
  const storage = resolveSessionStorage(config.storage, config.session)
  const client = createMtcuteRuntimeClient({ ...config, storage }, telemetry)

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
 * Call options for writes that may hit an account-wide limit: mtcute never
 * sleeps through a flood wait and resends, so a wait reaches the caller
 * and the flood guard saves it as a hold.
 */
export const NO_FLOOD_SLEEP: Readonly<RpcCallOptions> = {
  floodSleepThreshold: 0,
}

/**
 * Adds a user or bot peer to a Telegram chat. Never sleeps through a flood
 * wait (`NO_FLOOD_SLEEP`).
 */
export async function addTargetToChat(
  client: TelegramClient,
  params: TargetWriteParams,
): Promise<void> {
  await addChatMembers(
    client.withParams(NO_FLOOD_SLEEP),
    params.chatId,
    params.targetPeer,
    {},
  )
}

/**
 * Promotes a user or bot peer using the supplied admin-rights profile.
 * Promoting a non-member also adds them, so by default the call never
 * sleeps through a flood wait; pass `{}` for mtcute's usual flood sleep.
 */
export async function promoteTargetAdmin(
  client: TelegramClient,
  params: PromoteTargetParams,
  callParams: RpcCallOptions = NO_FLOOD_SLEEP,
): Promise<void> {
  await editAdminRights(client.withParams(callParams), {
    chatId: params.chatId,
    userId: params.targetPeer,
    rights: params.rights,
    rank: params.rank ?? "",
  })
}

/**
 * Parameters for `promoteExistingMember`.
 */
export interface PromoteExistingMemberParams extends PromoteTargetParams {
  /** Planned target user ID; the fresh read must resolve to this user. */
  expectedUserId?: number
}

/**
 * Promotes a peer only if it is already a chat member. Telegram adds a
 * non-member as a side effect of promotion, so this reads the target's
 * membership again just before the write and throws when the peer is absent,
 * left, banned, no longer a member, or a different user than planned. About
 * one RPC still separates that read from `channels.editAdmin`; Telegram has
 * no promote-without-add call to close the gap.
 */
export async function promoteExistingMember(
  client: TelegramClient,
  params: PromoteExistingMemberParams,
): Promise<void> {
  const member = await getTargetChatMember(client, {
    chatId: params.chatId,
    targetUserId: params.targetPeer,
  })
  assertPresentMember(
    member ? normalizeChatMember(member) : null,
    params.targetPeer,
    params.expectedUserId,
  )
  const { expectedUserId: _expected, ...promote } = params
  await promoteTargetAdmin(client, promote)
}

/**
 * Reads a chat's display title (the title for groups and channels).
 */
export async function fetchChatTitle(
  client: TelegramClient,
  chatId: number | string,
): Promise<string> {
  const chat = await client.getChat(chatId)
  return chat.displayName
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
