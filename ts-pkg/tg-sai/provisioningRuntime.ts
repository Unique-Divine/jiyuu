import { createHash, randomUUID } from "node:crypto"
import { mkdir, open, readFile, rename, rm } from "node:fs/promises"
import { dirname, isAbsolute, join, resolve } from "node:path"
import {
  addChatMembers,
  createInviteLink,
  createSupergroup,
  getChatMember,
  iterDialogs,
} from "@mtcute/bun/methods.js"
import { saiBot } from "./cfg"
import { createMtcuteClient, promoteTargetAdmin } from "./mtcuteAdapter"
import { configDir, strictProfileCredentials } from "./profiles"
import { PROVISIONING_BOT_ID } from "./provisioning"
import type {
  JournalRecord,
  Member,
  ProvisionJournal,
  ProvisionRequest,
  TelegramProvisioner,
} from "./provisioning"

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

export async function withProvisionTelegram<T>(
  request: ProvisionRequest,
  operation: (telegram: TelegramProvisioner) => Promise<T>,
): Promise<T> {
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
    })
    try {
      if ((await client.getMe()).id !== request.expectedUserId)
        throw new Error("Custody account identity mismatch")
      const bot = await client.getUser(saiBot.handle)
      if (bot.id !== PROVISIONING_BOT_ID || !bot.isBot)
        throw new Error("Configured Sai bot identity mismatch")
      const member = async (
        chatId: number,
        userId: number,
      ): Promise<Member> => {
        const value = await getChatMember(client, { chatId, userId })
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
              (entry): entry is [string, boolean] =>
                typeof entry[1] === "boolean",
            ),
          ),
        }
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
      const adapter: TelegramProvisioner = {
        self: async () => (await client.getMe()).id,
        resolve: async (peer) => {
          const user = await client.getUser(
            typeof peer === "string"
              ? /^[0-9]+$/.test(peer)
                ? Number(peer)
                : peer.replace(/^@/, "")
              : peer,
          )
          return { id: user.id, isBot: user.isBot }
        },
        create: async (title, marker) =>
          (
            await createSupergroup(client, {
              title,
              description: marker,
              forum: false,
            })
          ).id,
        member,
        add: async (chatId, userId) => {
          const missing = await addChatMembers(client, chatId, userId, {
            forwardCount: 0,
          })
          if (missing.length)
            throw Object.assign(new Error("Participant privacy restriction"), {
              text: "USER_PRIVACY_RESTRICTED",
            })
        },
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
        promote: async (chatId, userId, rights) =>
          promoteTargetAdmin(client, { chatId, targetPeer: userId, rights }),
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
      await client.disconnect()
    }
  })
}
