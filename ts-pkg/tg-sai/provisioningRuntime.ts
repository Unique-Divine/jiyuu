import { createHash, randomUUID } from "node:crypto"
import { mkdir, open, readFile, rename, rm } from "node:fs/promises"
import { dirname, isAbsolute, join, resolve } from "node:path"
import type { ChatMember } from "@mtcute/bun"
import {
  addChatMembers,
  createInviteLink,
  createSupergroup,
  getChatMember,
  getChatMembers,
  getPrimaryInviteLink,
  iterDialogs,
} from "@mtcute/bun/methods.js"
import { saiBot } from "./cfg"
import { createMtcuteClient, promoteTargetAdmin } from "./mtcuteAdapter"
import { configDir, strictProfileCredentials } from "./profiles"
import { PROVISIONING_BOT_ID } from "./provisioning"
import { withTelemetryIdentity } from "./telemetry/middleware"
import type { ClientTelemetry } from "./telemetry/middleware"
import type {
  JournalRecord,
  Member,
  ProvisionJournal,
  ProvisionRequest,
  ProvisioningLinks,
  TelegramProvisioner,
} from "./provisioning"

interface PrimaryInvite {
  link: string
  isPrimary: boolean
  isRevoked: boolean
  isMyLink: boolean
  endDate: Date | null
}
const COMPLETION_LINK_TIMEOUT_MS = 10_000

/** Fetch completion links without creating invitations or sending messages. */
export async function readProvisioningLinks(
  chatId: number,
  reader: {
    history(
      signal: AbortSignal,
    ): Promise<readonly { id: number; chat: { id: number } }[]>
    primaryInvite(signal: AbortSignal): Promise<PrimaryInvite>
  },
  timeoutMs = COMPLETION_LINK_TIMEOUT_MS,
): Promise<ProvisioningLinks> {
  // Bot API supergroup IDs encode the channel ID as -(10^12 + channel ID).
  const channelId = -chatId - 1_000_000_000_000
  if (!Number.isSafeInteger(chatId) || channelId <= 0) return {}
  const bounded = async <T>(
    read: (signal: AbortSignal) => Promise<T>,
  ): Promise<T> => {
    let timer: ReturnType<typeof setTimeout> | undefined
    const controller = new AbortController()
    try {
      return await Promise.race([
        Promise.resolve().then(() => read(controller.signal)),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => {
            controller.abort()
            reject(new Error("Completion link read timed out"))
          }, timeoutMs)
        }),
      ])
    } finally {
      clearTimeout(timer)
    }
  }
  const [history, invite] = await Promise.allSettled([
    bounded(reader.history),
    bounded(reader.primaryInvite),
  ])
  const result: ProvisioningLinks = {}
  if (history.status === "fulfilled") {
    const message = history.value.find(
      (value) =>
        value.chat.id === chatId &&
        Number.isSafeInteger(value.id) &&
        value.id > 0 &&
        value.id <= 2_147_483_647,
    )
    if (message) result.openGroupUrl = `https://t.me/c/${channelId}/${message.id}`
  }
  if (invite.status === "fulfilled") {
    const link = invite.value
    if (
      link.isPrimary &&
      link.isMyLink &&
      !link.isRevoked &&
      (!link.endDate || link.endDate.getTime() > Date.now()) &&
      /^https:\/\/t\.me\/(?:\+|joinchat\/)[A-Za-z0-9_-]{5,128}$/.test(link.link)
    )
      result.primaryInviteLink = link.link
  }
  return result
}

// Fresh custody sessions do not have access hashes for numeric user IDs yet.
// Handles discover the peer, but only the pinned numeric ID establishes identity.
const DEFAULT_MEMBER_HANDLES = new Map<number, string>([
  [1912700300, "uniquedivine"],
  [7930936303, "corinnebernett"],
  [5965882418, "lilnaptime"],
  [472528179, "erickpinos"],
])

export async function resolveProvisioningPeer(
  getUser: (peer: string | number) => Promise<{ id: number; isBot: boolean }>,
  peer: string | number,
): Promise<{ id: number; isBot: boolean }> {
  const normalized = typeof peer === "string" ? peer.replace(/^@/, "") : peer
  const numeric =
    typeof normalized === "number"
      ? normalized
      : /^[0-9]+$/.test(normalized)
        ? Number(normalized)
        : undefined
  const expectedId =
    numeric ??
    [...DEFAULT_MEMBER_HANDLES].find(
      ([, handle]) => handle.toLowerCase() === String(normalized).toLowerCase(),
    )?.[0]
  const lookup =
    expectedId === undefined
      ? normalized
      : (DEFAULT_MEMBER_HANDLES.get(expectedId) ?? expectedId)
  const user = await getUser(lookup)
  if (expectedId !== undefined && user.id !== expectedId)
    throw new Error("Provisioning participant identity mismatch")
  if (
    expectedId !== undefined &&
    DEFAULT_MEMBER_HANDLES.has(expectedId) &&
    user.isBot
  )
    throw new Error("Default provisioning participant must be a user account")
  return { id: user.id, isBot: user.isBot }
}

export function normalizeProvisioningMember(
  value: Pick<ChatMember, "status" | "isMember" | "permissions"> | null,
): Member {
  const role =
    value?.status === "creator"
      ? "creator"
      : value?.status === "admin"
        ? "admin"
        : value?.isMember
          ? "member"
          : "absent"
  return {
    role,
    rights: Object.fromEntries(
      Object.entries(value?.permissions ?? {}).filter(
        (entry): entry is [string, boolean] => typeof entry[1] === "boolean",
      ),
    ),
  }
}

/** A participant listing keyed by user ID; members that left are skipped. */
export function provisioningRoster(
  listed: readonly (Pick<ChatMember, "status" | "isMember" | "permissions"> & {
    user: { id: number }
  })[],
): Map<number, Member> {
  const roster = new Map<number, Member>()
  for (const value of listed) {
    const member = normalizeProvisioningMember(value)
    if (member.role !== "absent") roster.set(value.user.id, member)
  }
  return roster
}

/** Separate files and a process lock prevent account/session crossover. */
export function accountPaths(
  root: string,
  userId: number,
): { storage: string; lock: string } {
  if (!Number.isSafeInteger(userId) || userId <= 0)
    throw new Error("Invalid custody user ID")
  const base = resolve(root, "provisioning", "accounts", String(userId))
  return {
    storage: join(base, "mtcute.session"),
    lock: join(base, "worker.lock"),
  }
}
export async function withAccountLock<T>(
  lockPath: string,
  operation: () => Promise<T>,
): Promise<T> {
  if (!isAbsolute(lockPath))
    throw new Error("Absolute account lock path required")
  await mkdir(dirname(lockPath), { recursive: true, mode: 0o700 })
  // The kernel releases flock when this process dies. Persistent lock metadata
  // never authorizes deleting a live executor's lock or retrying group creation.
  if (!Bun.which("flock"))
    throw new Error("Custody worker requires the Linux flock utility")
  const guard = Bun.spawn(
    [
      "flock",
      "--exclusive",
      "--nonblock",
      lockPath,
      "sh",
      "-c",
      "printf 'locked\\n'; cat >/dev/null",
    ],
    {
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
    },
  )
  const reader = guard.stdout.getReader()
  const first = await reader.read()
  reader.releaseLock()
  if (first.done || new TextDecoder().decode(first.value).trim() !== "locked") {
    guard.stdin.end()
    await guard.exited
    throw new Error("Custody account is locked; another executor is active")
  }
  try {
    return await operation()
  } finally {
    guard.stdin.end()
    await guard.exited
  }
}

export class FileProvisionJournal implements ProvisionJournal {
  constructor(private root = configDir()) {}
  private path(requestId: string): string {
    return join(
      resolve(this.root),
      "provisioning",
      "receipts",
      `${createHash("sha256").update(requestId).digest("hex")}.json`,
    )
  }
  async read(requestId: string): Promise<JournalRecord | null> {
    try {
      return JSON.parse(
        await readFile(this.path(requestId), "utf8"),
      ) as JournalRecord
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return null
      throw new Error("Cannot read provisioning recovery receipt")
    }
  }
  async write(requestId: string, record: JournalRecord): Promise<void> {
    const path = this.path(requestId)
    await mkdir(dirname(path), { recursive: true, mode: 0o700 })
    const tmp = `${path}.${randomUUID()}.tmp`
    const file = await open(tmp, "wx", 0o600)
    try {
      await file.writeFile(JSON.stringify(record))
      await file.sync()
    } finally {
      await file.close()
    }
    await rename(tmp, path)
    const directory = await open(dirname(path), "r")
    try {
      await directory.sync()
    } finally {
      await directory.close()
    }
  }
}

/**
 * Runs `operation` with a provisioning adapter for the custody account.
 * When `signal` aborts, the client is destroyed: mtcute aborts its
 * `stopSignal`, which ends in-flight calls and flood-wait sleeps.
 */
export async function withProvisionTelegram<T>(
  request: Pick<ProvisionRequest, "custodyProfile" | "expectedUserId">,
  operation: (telegram: TelegramProvisioner) => Promise<T>,
  telemetry?: ClientTelemetry,
  signal?: AbortSignal,
): Promise<T> {
  signal?.throwIfAborted()
  const credentials = await strictProfileCredentials(
    request.custodyProfile,
    request.expectedUserId,
  )
  const paths = accountPaths(configDir(), request.expectedUserId)
  return withAccountLock(paths.lock, async () => {
    const client = await createMtcuteClient({
      apiId: credentials.apiId,
      apiHash: credentials.apiHash,
      session: credentials.sessionString,
      storage: paths.storage,
      // Keys the saved flood-hold guard to the custody account.
      accountId: request.expectedUserId,
    }, withTelemetryIdentity(telemetry, {
      accountId: request.expectedUserId,
      profile: request.custodyProfile,
    }))
    let destroying: Promise<void> | null = null
    const destroy = () => {
      destroying ??= client.destroy().catch(() => {})
    }
    signal?.addEventListener("abort", destroy, { once: true })
    try {
      signal?.throwIfAborted()
      if ((await client.getMe()).id !== request.expectedUserId)
        throw new Error("Custody account identity mismatch")
      const bot = await client.getUser(saiBot.handle)
      if (bot.id !== PROVISIONING_BOT_ID || !bot.isBot)
        throw new Error("Configured Sai bot identity mismatch")
      const member = async (
        chatId: number,
        userId: number,
      ): Promise<Member> => {
        return normalizeProvisioningMember(
          await getChatMember(client, { chatId, userId }),
        )
      }
      const verifyMarker = async (
        chatId: number,
        marker: string,
      ): Promise<boolean> => {
        const chat = await client.getFullChat(chatId)
        return (
          chat.chatType === "supergroup" &&
          !chat.isForum &&
          chat.isCreator &&
          chat.bio === marker
        )
      }
      const linkClient = (abortSignal: AbortSignal) => client.withParams({
        abortSignal,
        timeout: COMPLETION_LINK_TIMEOUT_MS,
        floodSleepThreshold: 0,
        maxRetryCount: 0,
      })
      // Create and add never sleep through a flood wait: the error reaches
      // provisioning, which saves its retry time and releases the worker.
      const writeClient = client.withParams({ floodSleepThreshold: 0 })
      const adapter: TelegramProvisioner = {
        self: async () => (await client.getMe()).id,
        resolve: (peer) =>
          resolveProvisioningPeer((value) => client.getUser(value), peer),
        create: async (title, marker) =>
          (
            await createSupergroup(writeClient, {
              title,
              description: marker,
              forum: false,
            })
          ).id,
        member,
        add: async (chatId, userId) => {
          const missing = await addChatMembers(writeClient, chatId, userId, {
            forwardCount: 0,
          })
          if (missing.length)
            throw Object.assign(new Error("Participant privacy restriction"), {
              text: "USER_PRIVACY_RESTRICTED",
            })
        },
        addMany: async (chatId, userIds) =>
          (
            await addChatMembers(writeClient, chatId, userIds, {
              forwardCount: 0,
            })
          ).map((invitee) => invitee.userId),
        inviteLink: async (chatId) => {
          const expires = new Date(Date.now() + 24 * 60 * 60 * 1000)
          const link = await createInviteLink(client, chatId, {
            expires,
            withApproval: true,
          })
          if (
            !link.isMyLink ||
            link.isRevoked ||
            !link.approvalNeeded ||
            !link.endDate
          ) {
            throw new Error(
              "Controlled invitation properties were not confirmed",
            )
          }
          return {
            inviteLink: link.link,
            expiresAt: link.endDate.toISOString(),
          }
        },
        completionLinks: (chatId) =>
          readProvisioningLinks(chatId, {
            history: (abortSignal) =>
              linkClient(abortSignal).getHistory(chatId, { limit: 1 }),
            primaryInvite: (abortSignal) =>
              getPrimaryInviteLink(linkClient(abortSignal), chatId),
          }),
        // Promotion keeps mtcute's flood sleep, as before.
        promote: async (chatId, userId, rights) =>
          promoteTargetAdmin(
            client,
            { chatId, targetPeer: userId, rights },
            {},
          ),
        members: async (chatId) => {
          // Deal chats stay far below Telegram's 200-member listing cap.
          const listed = await getChatMembers(client, chatId, {
            limit: 200,
            type: "recent",
          })
          return provisioningRoster(listed)
        },
        verifyMarker,
        candidates: async (marker) => {
          const candidates: number[] = []
          for await (const dialog of iterDialogs(client, {
            limit: 200,
            archived: "keep",
          })) {
            const chat = dialog.peer
            if (
              chat.type !== "chat" ||
              chat.chatType !== "supergroup" ||
              !chat.isCreator
            )
              continue
            if (await verifyMarker(chat.id, marker)) candidates.push(chat.id)
          }
          return candidates
        },
      }
      return await operation(adapter)
    } finally {
      signal?.removeEventListener("abort", destroy)
      if (destroying) await destroying
      else await client.disconnect()
    }
  })
}
