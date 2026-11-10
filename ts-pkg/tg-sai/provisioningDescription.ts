import {
  chatDescription,
  creationMarker,
  requestFingerprint,
} from "./provisioning"
import type {
  ChatInfo,
  JournalRecord,
  Setup,
  TelegramProvisioner,
} from "./provisioning"

export interface DescriptionRewrite {
  dryRun: boolean
  requestId: string
  chatId: number
  title: string
  currentDescription: string
  newDescription: string
  /** False when the chat already has the new description. */
  changed: boolean
}

/**
 * Moves one deal chat from the legacy `sai-setup:` description to
 * `chatDescription`. It refuses unless the custody account created the
 * chat and the current description is exactly this setup's legacy marker.
 * Without `run` it only reads. A chat already on the new description is
 * reported unchanged.
 */
export async function rewriteChatDescription(params: {
  setup: Setup
  local: JournalRecord | null
  telegram: Pick<
    TelegramProvisioner,
    "self" | "chatInfo" | "setDescription"
  >
  run: boolean
}): Promise<DescriptionRewrite> {
  const { setup, local, telegram } = params
  const fingerprint = requestFingerprint(setup)
  if (local && local.fingerprint !== fingerprint)
    throw new Error("Local receipt fingerprint conflicts with cloud request")
  if (setup.chatId && local?.chatId && setup.chatId !== local.chatId)
    throw new Error("Local receipt chat conflicts with cloud setup chat")
  const chatId = setup.chatId ?? local?.chatId
  if (!chatId) throw new Error("Setup has no chat to describe")
  if (!telegram.chatInfo || !telegram.setDescription)
    throw new Error("Telegram adapter cannot edit chat descriptions")
  if ((await telegram.self()) !== setup.expectedUserId)
    throw new Error("Custody account identity mismatch")
  const legacy = creationMarker(setup)
  const next = chatDescription(setup)
  const before = await telegram.chatInfo(chatId)
  if (!createdByCustody(before))
    throw new Error("Custody account did not create this supergroup")
  const result: DescriptionRewrite = {
    dryRun: !params.run,
    requestId: setup.requestId,
    chatId,
    title: before.title,
    currentDescription: before.description,
    newDescription: next,
    changed: before.description !== next,
  }
  if (!result.changed) return result
  if (before.description !== legacy)
    throw new Error(
      "Chat description is not this setup's legacy marker; refusing to rewrite",
    )
  if (!params.run) return result
  await telegram.setDescription(chatId, next)
  const after = await telegram.chatInfo(chatId)
  if (!createdByCustody(after) || after.description !== next)
    throw new Error("Chat description did not read back as written")
  return result
}

function createdByCustody(chat: ChatInfo): boolean {
  return chat.chatType === "supergroup" && !chat.isForum && chat.isCreator
}
