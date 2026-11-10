import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Command } from "commander"
import { getChatMember, iterDialogs } from "@mtcute/bun/methods.js"
import { createMtcuteClient } from "./mtcuteAdapter"
import { strictProfileCredentials } from "./profiles"
import { creationMarker, descriptionMatchesMarker, PROVISIONING_BOT_ID } from "./provisioning"
import { setupFromCloud } from "./provisioningCloud"
import { requireExactCaptureDialog, sanitizedProvisioningGolden } from "./provisioningGoldens"
import { markerChatMatches, resolveProvisioningPeer } from "./provisioningRuntime"

// Only read methods are used. An explicit persisted setup binds the target chat
// and identities; a fresh temporary store avoids touching worker session files.
export async function captureProvisioningGolden(options: {
  profile: string; chat: string; setup: string; out: string
}) {
  const chatId = Number(options.chat)
  const setup = setupFromCloud(JSON.parse(await readFile(options.setup, "utf8")))
  if (!Number.isSafeInteger(chatId) || chatId >= 0 || setup.chatId !== chatId ||
      setup.expectedUserId !== 8274304175 || !setup.binding ||
      setup.custodyProfile !== options.profile)
    throw new Error("Capture requires the exact custody setup and group")
  const credentials = await strictProfileCredentials(options.profile, setup.expectedUserId)
  const directory = await mkdtemp(join(tmpdir(), "tg-sai-golden-"))
  let client: Awaited<ReturnType<typeof createMtcuteClient>> | undefined
  let stage = "connect"
  try {
    client = await createMtcuteClient({ apiId: credentials.apiId,
      apiHash: credentials.apiHash, session: credentials.sessionString,
      storage: join(directory, "session") })
    stage = "self"
    const me = await client.getMe()
    if (me.id !== setup.expectedUserId || me.isBot)
      throw new Error("Custody identity mismatch")
    stage = "dialogs"
    await requireExactCaptureDialog(iterDialogs(client, {
      limit: 1000, archived: "keep",
    }), chatId)
    stage = "fullchat"
    const chat = await client.getFullChat(chatId)
    const marker = creationMarker(setup)
    console.log(JSON.stringify({ stage: "group_verified_readback", exactId: chat.id === chatId,
      chatType: chat.chatType, isForum: chat.isForum, isCreator: chat.isCreator,
      markerMatches: descriptionMatchesMarker(chat.bio, marker) }))
    if (chat.id !== chatId || !markerChatMatches(chat, marker))
      throw new Error("Capture group ownership or marker mismatch")
    const subjects = new Map<number, string>([
      [setup.expectedUserId, "custody"], [PROVISIONING_BOT_ID, "bot"],
    ])
    setup.binding.admins.forEach((id, i) => {
      if (!subjects.has(id)) subjects.set(id, `required_admin_${i}`)
    })
    setup.binding.participants.forEach((id, i) => {
      if (!subjects.has(id)) subjects.set(id, `ordinary_${i}`)
    })
    const members = []
    for (const [userId, subject] of subjects) {
      stage = `member_${subject}`
      if (subject.startsWith("ordinary_"))
        await resolveProvisioningPeer(peer => client!.getUser(peer), userId)
      const value = await getChatMember(client, { chatId, userId })
      members.push({ subject, value })
      console.log(JSON.stringify({ subject, status: value?.status ?? "absent" }))
    }
    stage = "invitations"
    const invites = await client.getInviteLinks(chatId, { limit: 100 })
    const golden = sanitizedProvisioningGolden({ capturedAt: new Date().toISOString(),
      chat, expectedMarker: marker, members, invites })
    await writeFile(options.out, `${JSON.stringify(golden, null, 2)}\n`, { flag: "wx", mode: 0o600 })
    return { members: members.length, invitations: invites.length }
  } catch (error) {
    const record = error && typeof error === "object" ? error as Record<string, unknown> : {}
    const code = typeof record.text === "string" && /^[A-Z0-9_]{1,100}$/.test(record.text)
      ? record.text : "unclassified"
    console.error(JSON.stringify({ stage, code }))
    throw new Error("Sanitized capture failure")
  } finally {
    try { await client?.disconnect() } finally { await rm(directory, { recursive: true, force: true }) }
  }
}

if (import.meta.main) {
  const command = new Command().description("Capture sanitized read-only evidence from one authorized custody group")
    .requiredOption("--profile <name>", "Exact configured custody profile")
    .requiredOption("--chat <id>", "Exact negative Telegram group ID")
    .requiredOption("--setup <path>", "Deal Desk setup readback JSON with resolved bindings")
    .requiredOption("--out <path>", "New sanitized fixture file; existing files are refused")
  command.parse()
  try {
    const counts = await captureProvisioningGolden(command.opts())
    console.log(`Saved sanitized readback: ${counts.members} members, ${counts.invitations} invitations`)
  } catch {
    console.error("Capture failed. Check the exact setup, custody identity, permissions, and destination. Raw Telegram errors are suppressed.")
    process.exitCode = 1
  }
}
