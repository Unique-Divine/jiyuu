import { describe, expect, test } from "bun:test"
import type { ChatMemberData } from "./adminAudit"
import { saiBot } from "./cfg"
import { registrationDiagnostics } from "./registrationAudit"

const supergroup = { id: -1001234567890, title: "Sai deal", type: "supergroup" } as const
const group = { id: -12345678, title: "Sai deal", type: "group" } as const
const rights = { inviteUsers: true, pinMessages: true }

function bot(status: ChatMemberData["status"], permissions: ChatMemberData["permissions"] = null): ChatMemberData {
  return {
    user: { id: saiBot.id, username: saiBot.handle, displayName: "SaiTeam", isBot: true, rawType: "user" },
    status,
    title: null,
    invitedBy: null,
    promotedBy: null,
    restrictedBy: null,
    restrictions: null,
    isMember: status !== "left",
    permissions,
    rawType: "chatParticipant",
  }
}

describe("registration diagnostics", () => {
  test("reads explicit supergroup administrator rights", () => {
    const result = registrationDiagnostics({
      chat: supergroup,
      ownerUserId: 12345,
      bot: bot("admin", { ...rights, other: true }),
      defaultMemberRights: null,
    })
    expect(result.chatId).toBe("-1001234567890")
    expect(result.ownerUserId).toBe("12345")
    expect(result.bot).toEqual({
      userId: String(saiBot.id),
      status: "admin",
      adminFlags: ["inviteUsers", "other", "pinMessages"],
      effectiveRights: rights,
      rightsSource: "admin",
    })
    expect(result.blockers).toEqual([])
  })

  test("a basic-group administrator has invite and pin rights with null rights", () => {
    const result = registrationDiagnostics({
      chat: group,
      ownerUserId: 12345,
      bot: bot("admin"),
      defaultMemberRights: { inviteUsers: false, pinMessages: false },
    })
    expect(result.bot.effectiveRights).toEqual(rights)
    expect(result.bot.rightsSource).toBe("basic_group_admin")
    expect(result.blockers).toEqual([])
  })

  test("group defaults describe a plain member but do not clear the admin gate", () => {
    const result = registrationDiagnostics({
      chat: group,
      ownerUserId: 12345,
      bot: bot("member"),
      defaultMemberRights: rights,
    })
    expect(result.bot.effectiveRights).toEqual(rights)
    expect(result.bot.rightsSource).toBe("group_default")
    expect(result.blockers).toEqual(["SaiTeam_bot must be a chat administrator"])
  })

  test("unseen membership does not inherit defaults", () => {
    const result = registrationDiagnostics({
      chat: group,
      ownerUserId: null,
      bot: null,
      defaultMemberRights: rights,
    })
    expect(result.bot.status).toBe("not_observed")
    expect(result.bot.effectiveRights).toBeNull()
    expect(result.blockers).toEqual([
      "Could not verify a single human chat owner",
      "SaiTeam_bot must be a chat administrator",
    ])
  })

  test("supergroup admin rights diagnostics surface missing and extra rights", () => {
    const result = registrationDiagnostics({
      chat: supergroup,
      ownerUserId: 12345,
      bot: bot("admin", { inviteUsers: true, changeInfo: true }),
      defaultMemberRights: rights,
    })
    expect(result.bot.effectiveRights).toEqual({ inviteUsers: true, pinMessages: false })
    expect(result.blockers).toEqual([
      "SaiTeam_bot needs verified administrator invite and pin rights",
      "Unexpected administrator rights: changeInfo",
    ])
  })
})
