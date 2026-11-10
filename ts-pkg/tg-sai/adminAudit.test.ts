import { describe, expect, test } from "bun:test"
import type {
  ChatAuditInput,
  ChatMemberData,
  PromoteOnlyOptions,
} from "./adminAudit"
import {
  assertPresentMember,
  canActorPromoteAdmins,
  canActorTransferOwnership,
  deriveMemberRole,
  isPresentMember,
  planAdminBootstrap,
  planPromoteOnly,
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
  corinneAntoniaUserId,
  exampleChatId,
  getChatMember_asMember,
  getChatMember_botMemberNibiruInternal,
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

describe("isPresentMember", () => {
  test("counts creators, admins, and members", () => {
    for (const status of ["creator", "admin", "member"]) {
      expect(isPresentMember(memberWith({ status }))).toBe(true)
    }
  })

  test("rejects absent, left, and banned users", () => {
    expect(isPresentMember(null)).toBe(false)
    expect(isPresentMember(memberWith({ status: "left" }))).toBe(false)
    expect(isPresentMember(memberWith({ status: "banned" }))).toBe(false)
  })

  test("counts restricted users only while still members", () => {
    const restricted = memberWith({ status: "restricted", isMember: true })
    expect(isPresentMember(restricted)).toBe(true)
    expect(isPresentMember({ ...restricted, isMember: false })).toBe(false)
  })
})

const team: PromoteOnlyOptions = {
  allowedUserIds: new Set([uniqueDivineUserId, corinneAntoniaUserId]),
}

function ownerWith(target: ChatMemberData | null): ChatAuditInput {
  return auditInput(memberWith({ status: "creator" }), target)
}

describe("planPromoteOnly", () => {
  test("promotes an existing team member when the actor is owner", () => {
    const plan = planPromoteOnly(adminCheck_asOwnerWithTargetMember.data, team)

    expect(plan.actorRole).toBe("creator")
    expect(plan.targetIsMember).toBe(true)
    expect(plan.canPromote).toBe(true)
    expect(plan.proposedActions).toEqual(["promote target user admin"])
  })

  test("refuses a target that is not in the chat and never proposes add", () => {
    const plan = planPromoteOnly(
      adminCheck_ownerSessionTargetUserAbsent.data,
      team,
    )

    expect(plan.actorIsOwner).toBe(true)
    expect(plan.targetRole).toBe("not_present")
    expect(plan.canPromote).toBe(false)
    expect(plan.proposedActions).toEqual(["refuse: target user not a member"])
  })

  test("refuses a target that left the chat", () => {
    const left = memberWith({ status: "left", isMember: false })
    const plan = planPromoteOnly(ownerWith(left), { allowedUserIds: null })

    expect(plan.proposedActions).toEqual(["refuse: target user not a member"])
  })

  test("refuses a bot member", () => {
    const plan = planPromoteOnly(
      ownerWith(getChatMember_botMemberNibiruInternal.data),
      { allowedUserIds: null },
    )

    expect(plan.targetIsBot).toBe(true)
    expect(plan.proposedActions).toEqual(["refuse: target user is a bot"])
  })

  test("refuses a member outside the team allowlist", () => {
    const outsider = memberWith({
      status: "member",
      user: { ...getChatMember_asMember.data.user, id: 424242 },
    })
    const plan = planPromoteOnly(ownerWith(outsider), team)

    expect(plan.targetIsAllowed).toBe(false)
    expect(plan.proposedActions).toEqual([
      "refuse: target user not on the team allowlist",
    ])
    expect(
      planPromoteOnly(ownerWith(outsider), { allowedUserIds: null })
        .proposedActions,
    ).toEqual(["promote target user admin"])
  })

  test("is a noop for a target that is already admin", () => {
    const plan = planPromoteOnly(adminCheck_asOwnerWithTargetAdmin.data, team)

    expect(plan.canPromote).toBe(false)
    expect(plan.proposedActions).toEqual(["noop: target user already admin"])
  })

  test("refuses a non-owner actor, even an admin with addAdmins", () => {
    const admin = memberWith({
      status: "admin",
      permissions: { addAdmins: true },
    })
    const target = adminCheck_asOwnerWithTargetMember.data.target
    const plan = planPromoteOnly(auditInput(admin, target), team)

    expect(plan.actorIsOwner).toBe(false)
    expect(plan.proposedActions).toEqual([
      "refuse: actor is not the chat owner",
    ])
    expect(
      planPromoteOnly(adminCheck_asMember.data, team).proposedActions,
    ).toEqual(["refuse: actor is not the chat owner"])
  })

  test("never proposes add or transfer actions", () => {
    const inputs = [
      adminCheck_asMember.data,
      adminCheck_asOwnerWithTargetMember.data,
      adminCheck_asOwnerWithTargetAdmin.data,
      adminCheck_ownerSessionTargetUserAbsent.data,
      adminCheck_botSessionOwnerTargetAbsent.data,
    ]
    for (const input of inputs) {
      for (const options of [team, { allowedUserIds: null }]) {
        for (const action of planPromoteOnly(input, options).proposedActions) {
          expect(action).not.toContain("add target")
          expect(action).not.toContain("transfer")
        }
      }
    }
  })
})

describe("assertPresentMember", () => {
  test("passes for a member and throws for an absent user", () => {
    expect(() =>
      assertPresentMember(getChatMember_targetAsMember.data, "@member"),
    ).not.toThrow()
    expect(() => assertPresentMember(null, "@absent")).toThrow(
      "Refusing to promote @absent: not a member of the chat",
    )
  })

  test("throws when the fresh read resolves to another user", () => {
    const member = getChatMember_targetAsMember.data
    expect(() =>
      assertPresentMember(member, "@member", member.user.id),
    ).not.toThrow()
    expect(() =>
      assertPresentMember(member, "@member", member.user.id + 1),
    ).toThrow("resolved to user")
  })
})
