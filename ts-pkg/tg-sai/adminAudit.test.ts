import { describe, expect, test } from "bun:test"
import type { ChatAuditInput, ChatMemberData } from "./adminAudit"
import {
  canActorPromoteAdmins,
  canActorTransferOwnership,
  deriveMemberRole,
  planAdminBootstrap,
} from "./adminAudit"
import {
  adminCheck_asMember,
  adminCheck_asOwnerWithTargetAdmin,
  adminCheck_asOwnerWithTargetMember,
  adminCheck_botSessionOwnerTargetAbsent,
  adminCheck_botSessionOwnerTargetAdmin,
  adminCheck_botSessionOwnerTargetMember,
  adminCheck_ownerSessionTargetUserAbsent,
  adminCheck_ownerSessionTargetUserAdmin,
  adminCheck_ownerSessionTargetUserMember,
  exampleChatId,
  getChatMember_asMember,
  getChatMember_targetAsMember,
  getMe,
  uniqueDivineUserId,
} from "./testdata"

function memberWith(overrides: Partial<ChatMemberData>): ChatMemberData {
  return {
    ...getChatMember_asMember.data,
    ...overrides,
  }
}

function auditInput(
  actor: ChatMemberData | null,
  target: ChatMemberData | null,
): ChatAuditInput {
  return {
    chatId: exampleChatId,
    targetUserId: uniqueDivineUserId,
    self: getMe.data,
    actor,
    target,
  }
}

describe("deriveMemberRole", () => {
  test("returns not_present for null members", () => {
    expect(deriveMemberRole(null)).toBe("not_present")
  })

  test("maps known Telegram member statuses", () => {
    expect(deriveMemberRole(memberWith({ status: "creator" }))).toBe("creator")
    expect(deriveMemberRole(memberWith({ status: "admin" }))).toBe("admin")
    expect(deriveMemberRole(memberWith({ status: "member" }))).toBe("member")
  })
})

describe("canActorPromoteAdmins", () => {
  test("allows creators", () => {
    const input = auditInput(memberWith({ status: "creator" }), null)

    expect(canActorPromoteAdmins(input)).toBe(true)
  })

  test("allows admins with addAdmins rights", () => {
    const input = auditInput(
      memberWith({
        status: "admin",
        permissions: { addAdmins: true },
      }),
      null,
    )

    expect(canActorPromoteAdmins(input)).toBe(true)
  })

  test("rejects admins without addAdmins rights", () => {
    const input = auditInput(
      memberWith({
        status: "admin",
        permissions: { addAdmins: false },
      }),
      null,
    )

    expect(canActorPromoteAdmins(input)).toBe(false)
  })
})

describe("canActorTransferOwnership", () => {
  test("allows only creators", () => {
    expect(
      canActorTransferOwnership(adminCheck_asOwnerWithTargetMember.data),
    ).toBe(true)
    expect(canActorTransferOwnership(adminCheck_asMember.data)).toBe(false)
  })
})

describe("planAdminBootstrap", () => {
  test("matches the captured regular-member fixture", () => {
    const result = planAdminBootstrap(adminCheck_asMember.data)

    expect(result.actorCanApply).toBe(false)
    expect(result.actorRole).toBe("member")
    expect(result.targetRole).toBe("member")
    expect(result.proposedActions).toEqual([
      "skip: actor lacks owner/admin rights",
    ])
  })

  test("promotes target when actor is creator and target is member", () => {
    const result = planAdminBootstrap(
      auditInput(
        memberWith({ status: "creator" }),
        getChatMember_targetAsMember.data,
      ),
    )

    expect(result.actorCanApply).toBe(true)
    expect(result.proposedActions).toEqual(["promote target user admin"])
  })

  test("captures Corinne owner happy path for admin promotion", () => {
    const result = planAdminBootstrap(adminCheck_asOwnerWithTargetMember.data)

    expect(result.actorRole).toBe("creator")
    expect(result.actorCanApply).toBe(true)
    expect(result.actorCanTransferOwnership).toBe(true)
    expect(result.targetCanReceiveOwnership).toBe(false)
    expect(result.proposedActions).toEqual(["promote target user admin"])
  })

  test("captures Corinne owner happy path for ownership-transfer precheck", () => {
    const result = planAdminBootstrap(adminCheck_asOwnerWithTargetAdmin.data)

    expect(result.actorRole).toBe("creator")
    expect(result.actorCanTransferOwnership).toBe(true)
    expect(result.targetRole).toBe("admin")
    expect(result.targetCanReceiveOwnership).toBe(true)
    expect(result.proposedActions).toEqual(["noop: target user already admin"])
  })

  test("promotes target when actor is admin with addAdmins rights", () => {
    const result = planAdminBootstrap(
      auditInput(
        memberWith({
          status: "admin",
          permissions: { addAdmins: true },
        }),
        getChatMember_targetAsMember.data,
      ),
    )

    expect(result.actorCanApply).toBe(true)
    expect(result.proposedActions).toEqual(["promote target user admin"])
  })

  test("skips when actor admin lacks addAdmins rights", () => {
    const result = planAdminBootstrap(
      auditInput(
        memberWith({
          status: "admin",
          permissions: { addAdmins: false },
        }),
        getChatMember_targetAsMember.data,
      ),
    )

    expect(result.actorCanApply).toBe(false)
    expect(result.proposedActions).toEqual([
      "skip: actor lacks owner/admin rights",
    ])
  })

  test("does nothing when target is already admin", () => {
    const result = planAdminBootstrap(
      auditInput(
        memberWith({ status: "creator" }),
        memberWith({
          status: "admin",
          permissions: { addAdmins: false },
        }),
      ),
    )

    expect(result.targetIsAdmin).toBe(true)
    expect(result.proposedActions).toEqual(["noop: target user already admin"])
  })

  test("adds then promotes missing target", () => {
    const result = planAdminBootstrap(
      auditInput(memberWith({ status: "creator" }), null),
    )

    expect(result.targetRole).toBe("not_present")
    expect(result.proposedActions).toEqual([
      "add target user",
      "promote target user admin",
    ])
  })

  test("captures target chat before add-bot", () => {
    const result = planAdminBootstrap(
      adminCheck_botSessionOwnerTargetAbsent.data,
    )

    expect(result.actorRole).toBe("creator")
    expect(result.targetRole).toBe("not_present")
    expect(result.actorCanApply).toBe(true)
    expect(result.actorCanTransferOwnership).toBe(true)
    expect(result.proposedActions).toEqual([
      "add target user",
      "promote target user admin",
    ])
  })

  test("captures target chat after bot add before promotion", () => {
    const result = planAdminBootstrap(
      adminCheck_botSessionOwnerTargetMember.data,
    )

    expect(result.actorRole).toBe("creator")
    expect(result.targetRole).toBe("member")
    expect(result.targetIsAdmin).toBe(false)
    expect(result.proposedActions).toEqual(["promote target user admin"])
  })

  test("captures target chat after bot admin promotion", () => {
    const result = planAdminBootstrap(
      adminCheck_botSessionOwnerTargetAdmin.data,
    )

    expect(result.actorRole).toBe("creator")
    expect(result.targetRole).toBe("admin")
    expect(result.targetIsAdmin).toBe(true)
    expect(result.targetCanReceiveOwnership).toBe(false)
    expect(result.proposedActions).toEqual(["noop: target user already admin"])
  })

  test("captures target chat before add-owner", () => {
    const result = planAdminBootstrap(
      adminCheck_ownerSessionTargetUserAbsent.data,
    )

    expect(result.actorRole).toBe("creator")
    expect(result.targetRole).toBe("not_present")
    expect(result.actorCanTransferOwnership).toBe(true)
    expect(result.targetCanReceiveOwnership).toBe(false)
    expect(result.proposedActions).toEqual([
      "add target user",
      "promote target user admin",
    ])
  })

  test("captures target chat add-owner after user add before promotion", () => {
    const result = planAdminBootstrap(
      adminCheck_ownerSessionTargetUserMember.data,
    )

    expect(result.actorRole).toBe("creator")
    expect(result.targetRole).toBe("member")
    expect(result.targetIsAdmin).toBe(false)
    expect(result.targetCanReceiveOwnership).toBe(false)
    expect(result.proposedActions).toEqual(["promote target user admin"])
  })

  test("captures target chat add-owner after admin promotion", () => {
    const result = planAdminBootstrap(
      adminCheck_ownerSessionTargetUserAdmin.data,
    )

    expect(result.actorRole).toBe("creator")
    expect(result.targetRole).toBe("admin")
    expect(result.targetIsAdmin).toBe(true)
    expect(result.targetCanReceiveOwnership).toBe(true)
    expect(result.proposedActions).toEqual(["noop: target user already admin"])
  })

  test("adds then promotes target that left", () => {
    const result = planAdminBootstrap(
      auditInput(
        memberWith({ status: "creator" }),
        memberWith({ status: "left", isMember: false }),
      ),
    )

    expect(result.targetRole).toBe("left")
    expect(result.proposedActions).toEqual([
      "add target user",
      "promote target user admin",
    ])
  })
})
