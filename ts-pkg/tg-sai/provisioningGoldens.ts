import type { ChatInviteLink, ChatMember, FullChat } from "@mtcute/bun"
import { descriptionMatchesMarker } from "./provisioning"

/** Numeric IDs need an access hash cached from an exact dialog readback. */
export async function requireExactCaptureDialog(
  dialogs: AsyncIterable<{ peer: { id: number } }>,
  chatId: number,
  limit = 1000,
): Promise<void> {
  let inspected = 0
  for await (const dialog of dialogs) {
    inspected++
    if (dialog.peer.id === chatId) return
    if (inspected >= limit) break
  }
  throw new Error("Exact capture group was not found in bounded custody dialogs")
}

export type MemberShape = Pick<ChatMember, "status" | "isMember" | "permissions">
export type InviteShape = Pick<ChatInviteLink,
  "isMyLink" | "isRevoked" | "approvalNeeded" | "isPrimary" | "endDate" |
  "usageLimit" | "usage" | "pendingApprovals">

/** Copy only fields consumed by the adapter. Never serialize Telegram objects. */
export function memberShape(value: MemberShape | null): MemberShape | null {
  if (!value) return null
  return {
    status: value.status,
    isMember: value.isMember,
    permissions: value.permissions === null ? null : {
      _: "chatAdminRights",
      ...Object.fromEntries(Object.entries(value.permissions).filter(
        ([, v]) => typeof v === "boolean",
      )),
    },
  }
}

export function sanitizedProvisioningGolden(input: {
  capturedAt: string
  chat: Pick<FullChat, "chatType" | "isForum" | "isCreator" | "bio">
  expectedMarker: string
  members: { subject: string; value: MemberShape | null }[]
  invites: InviteShape[]
}) {
  return {
    format: "tg-sai-provisioning-golden/v1" as const,
    provenance: { kind: "live-readback" as const, capturedAt: input.capturedAt },
    chat: {
      chatType: input.chat.chatType,
      isForum: input.chat.isForum,
      isCreator: input.chat.isCreator,
      markerMatches: descriptionMatchesMarker(input.chat.bio, input.expectedMarker),
    },
    members: input.members.map(({ subject, value }) => ({ subject, value: memberShape(value) })),
    invitations: {
      limit: 100,
      mayHaveMore: input.invites.length === 100,
      entries: input.invites.map(invite => ({
        isMyLink: invite.isMyLink,
        isRevoked: invite.isRevoked,
        approvalNeeded: invite.approvalNeeded,
        isPrimary: invite.isPrimary,
        expiresAt: invite.endDate?.toISOString() ?? null,
        usageLimit: Number.isFinite(invite.usageLimit) ? invite.usageLimit : null,
        usage: invite.usage,
        pendingApprovals: invite.pendingApprovals,
      })),
    },
  }
}
