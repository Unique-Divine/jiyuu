export interface TestData<T> {
  goldReq: string
  notes?: string
  data: T
}

export interface UserData {
  id: number
  username: string | null
  displayName: string
  isBot: boolean
  rawType: string
}

export interface ChatMemberData {
  user: UserData
  status: string
  title: string | null
  invitedBy: UserData | null
  promotedBy: UserData | null
  restrictedBy: UserData | null
  restrictions: unknown | null
  isMember: boolean
  permissions: AdminPermissionsData | null
  rawType: string
}

export interface AdminPermissionsData {
  addAdmins?: boolean
  add_admins?: boolean
  [key: string]: unknown
}

export interface ChatAuditInput {
  chatId: number | string
  targetUserId: number | string
  self: UserData
  actor: ChatMemberData | null
  target: ChatMemberData | null
}

export interface ChatAuditResult {
  chatId: number | string
  targetUserId: number | string
  actorRole: MemberRole
  targetRole: MemberRole
  actorCanApply: boolean
  actorCanTransferOwnership: boolean
  targetCanReceiveOwnership: boolean
  targetIsAdmin: boolean
  proposedActions: ProposedAction[]
}

export type MemberRole =
  | "creator"
  | "admin"
  | "member"
  | "restricted"
  | "banned"
  | "left"
  | "not_present"
  | "unknown"

export type ProposedAction =
  | "skip: actor lacks owner/admin rights"
  | "add target user"
  | "promote target user admin"
  | "noop: target user already admin"

export function deriveMemberRole(member: ChatMemberData | null): MemberRole {
  if (!member) {
    return "not_present"
  }
  switch (member.status) {
    case "creator":
    case "admin":
    case "member":
    case "restricted":
    case "banned":
    case "left":
      return member.status
    default:
      return "unknown"
  }
}

export function hasAddAdminsRight(
  permissions: AdminPermissionsData | null,
): boolean {
  if (!permissions) {
    return false
  }
  return Boolean(permissions.addAdmins || permissions.add_admins)
}

export function canActorPromoteAdmins(input: ChatAuditInput): boolean {
  const actorRole = deriveMemberRole(input.actor)
  if (actorRole === "creator") {
    return true
  }
  if (actorRole !== "admin") {
    return false
  }
  return hasAddAdminsRight(input.actor?.permissions ?? null)
}

export function canActorTransferOwnership(input: ChatAuditInput): boolean {
  return deriveMemberRole(input.actor) === "creator"
}

export function isAdminRole(role: MemberRole): boolean {
  return role === "admin" || role === "creator"
}

export function planAdminBootstrap(input: ChatAuditInput): ChatAuditResult {
  const actorRole = deriveMemberRole(input.actor)
  const targetRole = deriveMemberRole(input.target)
  const actorCanApply = canActorPromoteAdmins(input)
  const actorCanTransferOwnership = canActorTransferOwnership(input)
  const targetIsAdmin = isAdminRole(targetRole)
  const targetCanReceiveOwnership =
    targetRole === "admin" && input.target?.user.isBot !== true

  const proposedActions: ProposedAction[] = []
  if (!actorCanApply) {
    proposedActions.push("skip: actor lacks owner/admin rights")
  } else if (targetIsAdmin) {
    proposedActions.push("noop: target user already admin")
  } else {
    if (targetRole === "not_present" || targetRole === "left") {
      proposedActions.push("add target user")
    }
    proposedActions.push("promote target user admin")
  }

  return {
    chatId: input.chatId,
    targetUserId: input.targetUserId,
    actorRole,
    targetRole,
    actorCanApply,
    actorCanTransferOwnership,
    targetCanReceiveOwnership,
    targetIsAdmin,
    proposedActions,
  }
}

/**
 * Reports whether a chat member is currently in the chat. Restricted users
 * count only while Telegram still marks them as members.
 */
export function isPresentMember(member: ChatMemberData | null): boolean {
  switch (deriveMemberRole(member)) {
    case "creator":
    case "admin":
    case "member":
      return true
    case "restricted":
      return member?.isMember === true
    default:
      return false
  }
}

export type PromoteOnlyAction =
  | "refuse: actor is not the chat owner"
  | "refuse: target user not a member"
  | "refuse: target user is a bot"
  | "refuse: target user not on the team allowlist"
  | "promote target user admin"
  | "noop: target user already admin"

export interface PromoteOnlyOptions {
  /**
   * User IDs the command may promote. `null` disables the check; the CLI
   * passes `null` only with flag `--allow-non-team`.
   */
  allowedUserIds: ReadonlySet<number> | null
}

export interface PromoteOnlyPlan {
  chatId: number | string
  targetUserId: number | string
  actorRole: MemberRole
  targetRole: MemberRole
  actorIsOwner: boolean
  targetIsMember: boolean
  targetIsAdmin: boolean
  targetIsBot: boolean
  /** True when the allowlist is off or contains the resolved target ID. */
  targetIsAllowed: boolean
  /** True only when the single proposed action is a promotion. */
  canPromote: boolean
  proposedActions: PromoteOnlyAction[]
}

/**
 * Plans a promote-only update: promote an existing chat member to admin.
 * Never proposes adding the target and never transfers ownership.
 *
 * The promotion grants owner-target rights, which only the chat owner can
 * grant in full, so a non-owner actor is refused. Bots are refused because
 * they go through command `add-bot` with the bot rights. Targets outside
 * `allowedUserIds` are refused unless the allowlist is `null`.
 */
export function planPromoteOnly(
  input: ChatAuditInput,
  options: PromoteOnlyOptions,
): PromoteOnlyPlan {
  const actorRole = deriveMemberRole(input.actor)
  const targetRole = deriveMemberRole(input.target)
  const actorIsOwner = actorRole === "creator"
  const targetIsMember = isPresentMember(input.target)
  const targetIsAdmin = isAdminRole(targetRole)
  const targetIsBot = input.target?.user.isBot === true
  const targetIsAllowed =
    options.allowedUserIds === null ||
    (input.target !== null && options.allowedUserIds.has(input.target.user.id))

  let action: PromoteOnlyAction
  if (!actorIsOwner) {
    action = "refuse: actor is not the chat owner"
  } else if (!targetIsMember) {
    action = "refuse: target user not a member"
  } else if (targetIsBot) {
    action = "refuse: target user is a bot"
  } else if (!targetIsAllowed) {
    action = "refuse: target user not on the team allowlist"
  } else if (targetIsAdmin) {
    action = "noop: target user already admin"
  } else {
    action = "promote target user admin"
  }

  return {
    chatId: input.chatId,
    targetUserId: input.targetUserId,
    actorRole,
    targetRole,
    actorIsOwner,
    targetIsMember,
    targetIsAdmin,
    targetIsBot,
    targetIsAllowed,
    canPromote: action === "promote target user admin",
    proposedActions: [action],
  }
}

/**
 * Throws unless the member is present in the chat and, when
 * `expectedUserId` is given, is that user. Telegram adds a non-member when
 * it promotes them, so promote-only writes call this on a fresh read just
 * before the promotion.
 */
export function assertPresentMember(
  member: ChatMemberData | null,
  targetLabel: number | string,
  expectedUserId?: number,
): void {
  if (!isPresentMember(member)) {
    throw new Error(
      `Refusing to promote ${targetLabel}: not a member of the chat ` +
        `(role: ${deriveMemberRole(member)})`,
    )
  }
  if (expectedUserId !== undefined && member?.user.id !== expectedUserId) {
    throw new Error(
      `Refusing to promote ${targetLabel}: resolved to user ` +
        `${member?.user.id}, expected ${expectedUserId}`,
    )
  }
}
