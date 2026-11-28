import type { ChatMemberData } from "./adminAudit"
import { saiBot } from "./cfg"

const BASELINE_ADMIN_FLAGS = ["inviteUsers", "pinMessages", "other"]

interface RegistrationRights {
  inviteUsers: boolean
  pinMessages: boolean
}

interface RegistrationAuditInput {
  chat: { id: number; title: string; type: "group" | "supergroup" }
  ownerUserId: number | null
  bot: ChatMemberData | null
  defaultMemberRights: RegistrationRights | null
}

export function registrationDiagnostics(input: RegistrationAuditInput) {
  const { chat, ownerUserId, bot, defaultMemberRights } = input
  const verifiedBot = bot?.user.id === saiBot.id && bot.user.isBot
  const status = verifiedBot ? bot.status : "not_observed"
  const adminRights = status === "admin" ? bot?.permissions : null
  const adminFlags = Object.entries(adminRights ?? {})
    .filter(([, enabled]) => enabled === true)
    .map(([right]) => right)
    .sort()

  let effectiveRights: RegistrationRights | null = null
  let rightsSource = "unknown"
  if (status === "admin" && chat.type === "group") {
    effectiveRights = { inviteUsers: true, pinMessages: true }
    rightsSource = "basic_group_admin"
  } else if (adminRights) {
    effectiveRights = {
      inviteUsers: adminRights.inviteUsers === true,
      pinMessages: adminRights.pinMessages === true,
    }
    rightsSource = "admin"
  } else if (status === "member" && defaultMemberRights) {
    effectiveRights = defaultMemberRights
    rightsSource = "group_default"
  }

  const blockers: string[] = []
  if (ownerUserId === null) {
    blockers.push("Could not verify a single human chat owner")
  }
  if (status !== "admin") {
    blockers.push("SaiTeam_bot must be a chat administrator")
  } else if (!effectiveRights?.inviteUsers || !effectiveRights.pinMessages) {
    blockers.push("SaiTeam_bot needs verified administrator invite and pin rights")
  }
  if (chat.type === "supergroup") {
    const extra = adminFlags.filter(
      (right) => !BASELINE_ADMIN_FLAGS.includes(right),
    )
    if (extra.length > 0) {
      blockers.push(`Unexpected administrator rights: ${extra.join(", ")}`)
    }
  }

  return {
    chatId: String(chat.id),
    title: chat.title,
    chatType: chat.type,
    ownerUserId: ownerUserId === null ? null : String(ownerUserId),
    bot: {
      userId: String(saiBot.id),
      status,
      adminFlags,
      effectiveRights,
      rightsSource,
    },
    defaultMemberRights,
    blockers,
  }
}
