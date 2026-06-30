export interface TestData<T> {
  goldReq: string;
  notes?: string;
  data: T;
}

export interface UserData {
  id: number;
  username: string | null;
  displayName: string;
  isBot: boolean;
  rawType: string;
}

export interface ChatMemberData {
  user: UserData;
  status: string;
  title: string | null;
  invitedBy: UserData | null;
  promotedBy: UserData | null;
  restrictedBy: UserData | null;
  restrictions: unknown | null;
  isMember: boolean;
  permissions: AdminPermissionsData | null;
  rawType: string;
}

export interface AdminPermissionsData {
  addAdmins?: boolean;
  add_admins?: boolean;
  [key: string]: unknown;
}

export interface ChatAuditInput {
  chatId: number | string;
  targetUserId: number | string;
  self: UserData;
  actor: ChatMemberData | null;
  target: ChatMemberData | null;
}

export interface ChatAuditResult {
  chatId: number | string;
  targetUserId: number | string;
  actorRole: MemberRole;
  targetRole: MemberRole;
  actorCanApply: boolean;
  actorCanTransferOwnership: boolean;
  targetCanReceiveOwnership: boolean;
  targetIsAdmin: boolean;
  proposedActions: ProposedAction[];
}

export type MemberRole =
  | "creator"
  | "admin"
  | "member"
  | "restricted"
  | "banned"
  | "left"
  | "not_present"
  | "unknown";

export type ProposedAction =
  | "skip: actor lacks owner/admin rights"
  | "add target user"
  | "promote target user admin"
  | "noop: target user already admin";

export function deriveMemberRole(member: ChatMemberData | null): MemberRole {
  if (!member) {
    return "not_present";
  }
  switch (member.status) {
    case "creator":
    case "admin":
    case "member":
    case "restricted":
    case "banned":
    case "left":
      return member.status;
    default:
      return "unknown";
  }
}

export function hasAddAdminsRight(
  permissions: AdminPermissionsData | null,
): boolean {
  if (!permissions) {
    return false;
  }
  return Boolean(permissions.addAdmins || permissions.add_admins);
}

export function canActorPromoteAdmins(input: ChatAuditInput): boolean {
  const actorRole = deriveMemberRole(input.actor);
  if (actorRole === "creator") {
    return true;
  }
  if (actorRole !== "admin") {
    return false;
  }
  return hasAddAdminsRight(input.actor?.permissions ?? null);
}

export function canActorTransferOwnership(input: ChatAuditInput): boolean {
  return deriveMemberRole(input.actor) === "creator";
}

export function isAdminRole(role: MemberRole): boolean {
  return role === "admin" || role === "creator";
}

export function planAdminBootstrap(input: ChatAuditInput): ChatAuditResult {
  const actorRole = deriveMemberRole(input.actor);
  const targetRole = deriveMemberRole(input.target);
  const actorCanApply = canActorPromoteAdmins(input);
  const actorCanTransferOwnership = canActorTransferOwnership(input);
  const targetIsAdmin = isAdminRole(targetRole);
  const targetCanReceiveOwnership =
    targetRole === "admin" && input.target?.user.isBot !== true;

  const proposedActions: ProposedAction[] = [];
  if (!actorCanApply) {
    proposedActions.push("skip: actor lacks owner/admin rights");
  } else if (targetIsAdmin) {
    proposedActions.push("noop: target user already admin");
  } else {
    if (targetRole === "not_present" || targetRole === "left") {
      proposedActions.push("add target user");
    }
    proposedActions.push("promote target user admin");
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
  };
}
