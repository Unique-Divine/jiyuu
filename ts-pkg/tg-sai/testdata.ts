import type {
  ChatAuditInput,
  ChatMemberData,
  TestData,
  UserData,
} from "./adminAudit"
import { saiBot, USERS } from "./cfg"

/**
 * Example Telegram basic-group chat ID used for captured admin-audit fixtures.
 * The chat exposed the MCP `get_admins` false-positive behavior that motivated
 * the mtcute and Telethon comparison.
 */
export const exampleChatId = -5064008855

/**
 * Supergroup fixture source where the session user is the creator and the
 * target bot was not present before add-bot. This is the owner-session path for bot add
 * and promote planning.
 */
export const chatNibiruInternalId = -1002094035214

export const uniqueDivineUserId = USERS.uniqueDivine.id

export const corinneUserId = USERS.corinne.id

export const saiBotUserId = saiBot.id

export const corinneAntoniaUserId = USERS.corinneAlt.id

export const corinneAntoniaUser: TestData<UserData> = {
  goldReq: "resolve_username('corinneantonia')",
  notes:
    "PoC target user for the target-chat owner transfer flow. This fixture records the concrete target account resolved for the experiment.",
  data: {
    id: corinneAntoniaUserId,
    username: USERS.corinneAlt.handle,
    displayName: USERS.corinneAlt.displayName,
    isBot: false,
    rawType: "user",
  },
}

export const getMe: TestData<UserData> = {
  goldReq: "client.getMe",
  notes: "Live mtcute result for the authenticated Telegram session.",
  data: {
    id: uniqueDivineUserId,
    username: USERS.uniqueDivine.handle,
    displayName: "Unique (NIBIRU) 🇺🇸🩵",
    isBot: false,
    rawType: "user",
  },
}

export const sessionUserUnique: TestData<UserData> = {
  goldReq: "client.getMe",
  notes:
    "Live mtcute result for the authenticated session user used on the target chat.",
  data: getMe.data,
}

export const getMe_asOwner: TestData<UserData> = {
  goldReq: "client.getMe",
  notes:
    "Fixture for a chat owner account. This models the happy path where the authenticated session is the chat creator.",
  data: {
    id: corinneUserId,
    username: USERS.corinne.handle,
    displayName: USERS.corinne.displayName,
    isBot: false,
    rawType: "user",
  },
}

export const getChatMember_asMember: TestData<ChatMemberData> = {
  goldReq: 'getChatMember(client, { chatId, userId: "self" })',
  notes:
    "Basic-group membership for the authenticated user; confirms this account is a regular member, not admin or creator.",
  data: {
    user: getMe.data,
    status: "member",
    title: null,
    invitedBy: {
      id: corinneUserId,
      username: USERS.corinne.handle,
      displayName: USERS.corinne.displayName,
      isBot: false,
      rawType: "user",
    },
    promotedBy: null,
    restrictedBy: null,
    restrictions: null,
    isMember: true,
    permissions: null,
    rawType: "chatParticipant",
  },
}

export const getChatMember_asOwner: TestData<ChatMemberData> = {
  goldReq: 'getChatMember(client, { chatId, userId: "self" })',
  notes:
    "Happy-path fixture where the authenticated session is the basic-group creator.",
  data: {
    user: getMe_asOwner.data,
    status: "creator",
    title: null,
    invitedBy: null,
    promotedBy: null,
    restrictedBy: null,
    restrictions: null,
    isMember: true,
    permissions: null,
    rawType: "chatParticipantCreator",
  },
}

export const getChatMember_sessionOwnerNibiruInternal: TestData<ChatMemberData> =
  {
    goldReq: 'getChatMember(client, { chatId, userId: "self" })',
    notes:
      "Target-chat supergroup membership for the session user; the active session is the chat creator with invite and add-admin rights.",
    data: {
      user: sessionUserUnique.data,
      status: "creator",
      title: null,
      invitedBy: null,
      promotedBy: null,
      restrictedBy: null,
      restrictions: null,
      isMember: true,
      permissions: {
        _: "chatAdminRights",
        changeInfo: true,
        postMessages: true,
        editMessages: true,
        deleteMessages: true,
        banUsers: true,
        inviteUsers: true,
        pinMessages: true,
        addAdmins: true,
        anonymous: false,
        manageCall: true,
        other: true,
        manageTopics: true,
        postStories: true,
        editStories: true,
        deleteStories: true,
        manageDirectMessages: true,
        manageRanks: true,
      },
      rawType: "channelParticipantCreator",
    },
  }

export const getChatMember_targetAsMember: TestData<ChatMemberData> = {
  goldReq: "getChatMember(client, { chatId, userId: uniqueDivineUserId })",
  notes:
    "Basic-group membership for the target user; same Telegram user as the actor in this fixture.",
  data: getChatMember_asMember.data,
}

export const getChatMember_targetAsAdmin: TestData<ChatMemberData> = {
  goldReq: "getChatMember(client, { chatId, userId: uniqueDivineUserId })",
  notes:
    "Fixture for the target user after promotion to admin, which is a prerequisite for ownership transfer flows.",
  data: {
    ...getChatMember_asMember.data,
    status: "admin",
    permissions: null,
    rawType: "chatParticipantAdmin",
  },
}

export const getChatMember_botMemberNibiruInternal: TestData<ChatMemberData> = {
  goldReq:
    "getChatMember(client, { chatId: chatNibiruInternalId, userId: saiBotUserId })",
  notes:
    "Modeled target-chat midpoint after the target bot is added but before admin promotion. The original live POC added and promoted in one run, so this transient state was not captured before promotion.",
  data: {
    user: {
      id: saiBotUserId,
      username: saiBot.handle,
      displayName: "Sai Team 🔥",
      isBot: true,
      rawType: "user",
    },
    status: "member",
    title: null,
    invitedBy: sessionUserUnique.data,
    promotedBy: null,
    restrictedBy: null,
    restrictions: null,
    isMember: true,
    permissions: null,
    rawType: "channelParticipant",
  },
}

export const getChatMember_botAdminNibiruInternal: TestData<ChatMemberData> = {
  goldReq:
    "getChatMember(client, { chatId: chatNibiruInternalId, userId: saiBotUserId })",
  notes:
    "Target-chat state after add-bot: the target bot is an admin but cannot receive Telegram ownership.",
  data: {
    user: {
      id: saiBotUserId,
      username: saiBot.handle,
      displayName: "Sai Team 🔥",
      isBot: true,
      rawType: "user",
    },
    status: "admin",
    title: null,
    invitedBy: null,
    promotedBy: sessionUserUnique.data,
    restrictedBy: null,
    restrictions: null,
    isMember: true,
    permissions: {
      _: "chatAdminRights",
      changeInfo: false,
      postMessages: false,
      editMessages: false,
      deleteMessages: true,
      banUsers: true,
      inviteUsers: true,
      pinMessages: true,
      addAdmins: false,
      anonymous: false,
      manageCall: false,
      other: true,
      manageTopics: true,
      postStories: false,
      editStories: false,
      deleteStories: false,
      manageDirectMessages: false,
      manageRanks: false,
    },
    rawType: "channelParticipantAdmin",
  },
}

export const getChatMember_ownerTargetMemberNibiruInternal: TestData<ChatMemberData> =
  {
    goldReq: "modeled from add-owner checkpoint target_role_after_add=member",
    notes:
      "Modeled target-chat midpoint after the target user is added but before admin promotion. The live add-owner run printed target_role_after_add=member, then promoted before a full normalized capture.",
    data: {
      user: corinneAntoniaUser.data,
      status: "member",
      title: null,
      invitedBy: sessionUserUnique.data,
      promotedBy: null,
      restrictedBy: null,
      restrictions: null,
      isMember: true,
      permissions: null,
      rawType: "channelParticipant",
    },
  }

export const getChatMember_ownerTargetAdminNibiruInternal: TestData<ChatMemberData> =
  {
    goldReq:
      "getChatMember(client, { chatId: chatNibiruInternalId, userId: corinneAntoniaUserId })",
    notes:
      "Live target-chat result after the target user was promoted to admin by the session user.",
    data: {
      user: corinneAntoniaUser.data,
      status: "admin",
      title: null,
      invitedBy: null,
      promotedBy: sessionUserUnique.data,
      restrictedBy: null,
      restrictions: null,
      isMember: true,
      permissions: {
        _: "chatAdminRights",
        changeInfo: true,
        postMessages: false,
        editMessages: false,
        deleteMessages: true,
        banUsers: true,
        inviteUsers: true,
        pinMessages: true,
        addAdmins: true,
        anonymous: false,
        manageCall: true,
        other: true,
        manageTopics: true,
        postStories: false,
        editStories: false,
        deleteStories: false,
        manageDirectMessages: false,
        manageRanks: true,
      },
      rawType: "channelParticipantAdmin",
    },
  }

export const adminCheck_asMember: TestData<ChatAuditInput> = {
  goldReq: "bun run adminCheck.ts",
  notes:
    "Combined read-only proof-of-concept result where the authenticated user is a regular member.",
  data: {
    chatId: exampleChatId,
    targetUserId: uniqueDivineUserId,
    self: getMe.data,
    actor: getChatMember_asMember.data,
    target: getChatMember_targetAsMember.data,
  },
}

export const adminCheck_asOwnerWithTargetMember: TestData<ChatAuditInput> = {
  goldReq: "bun run adminCheck.ts",
  notes:
    "Owner-session fixture where the session user is the chat creator and the target user is a regular member.",
  data: {
    chatId: exampleChatId,
    targetUserId: uniqueDivineUserId,
    self: getMe_asOwner.data,
    actor: getChatMember_asOwner.data,
    target: getChatMember_targetAsMember.data,
  },
}

export const adminCheck_asOwnerWithTargetAdmin: TestData<ChatAuditInput> = {
  goldReq: "bun run adminCheck.ts",
  notes:
    "Owner-session fixture where the session user is creator and the target user is already admin, allowing an ownership-transfer precheck to pass.",
  data: {
    chatId: exampleChatId,
    targetUserId: uniqueDivineUserId,
    self: getMe_asOwner.data,
    actor: getChatMember_asOwner.data,
    target: getChatMember_targetAsAdmin.data,
  },
}

export const adminCheck_botSessionOwnerTargetAbsent: TestData<ChatAuditInput> =
  {
    goldReq:
      "TELEGRAM_CHAT_ID=-1002094035214 TELEGRAM_TARGET_USER_ID=8983431101 bun run adminCheck.ts",
    notes:
      "Target chat before add-bot: the session user is the owner and the target bot is not present in the chat.",
    data: {
      chatId: chatNibiruInternalId,
      targetUserId: saiBotUserId,
      self: sessionUserUnique.data,
      actor: getChatMember_sessionOwnerNibiruInternal.data,
      target: null,
    },
  }

export const adminCheck_botSessionOwnerTargetMember: TestData<ChatAuditInput> =
  {
    goldReq:
      "TELEGRAM_CHAT_ID=-1002094035214 TELEGRAM_TARGET_USER_ID=8983431101 bun run adminCheck.ts",
    notes:
      "Target-chat midpoint model: the session user is the owner and the target bot is present as a non-admin member.",
    data: {
      chatId: chatNibiruInternalId,
      targetUserId: saiBotUserId,
      self: sessionUserUnique.data,
      actor: getChatMember_sessionOwnerNibiruInternal.data,
      target: getChatMember_botMemberNibiruInternal.data,
    },
  }

export const adminCheck_botSessionOwnerTargetAdmin: TestData<ChatAuditInput> = {
  goldReq:
    "TELEGRAM_CHAT_ID=-1002094035214 TELEGRAM_TARGET_USER_ID=8983431101 bun run adminCheck.ts",
  notes:
    "Target chat after bot admin promotion: the session user is the owner and the target bot is already admin.",
  data: {
    chatId: chatNibiruInternalId,
    targetUserId: saiBotUserId,
    self: sessionUserUnique.data,
    actor: getChatMember_sessionOwnerNibiruInternal.data,
    target: getChatMember_botAdminNibiruInternal.data,
  },
}

export const adminCheck_ownerSessionTargetUserAbsent: TestData<ChatAuditInput> =
  {
    goldReq:
      "just tg-sai add-owner --chat -1002094035214 --target-user @corinneantonia",
    notes:
      "Target chat before add-owner: the session user is the owner and the target user is not present in the chat.",
    data: {
      chatId: chatNibiruInternalId,
      targetUserId: corinneAntoniaUserId,
      self: sessionUserUnique.data,
      actor: getChatMember_sessionOwnerNibiruInternal.data,
      target: null,
    },
  }

export const adminCheck_ownerSessionTargetUserMember: TestData<ChatAuditInput> =
  {
    goldReq: "modeled from add-owner checkpoint target_role_after_add=member",
    notes:
      "Target-chat midpoint model: the session user is the owner and the target user is present as a non-admin member.",
    data: {
      chatId: chatNibiruInternalId,
      targetUserId: corinneAntoniaUserId,
      self: sessionUserUnique.data,
      actor: getChatMember_sessionOwnerNibiruInternal.data,
      target: getChatMember_ownerTargetMemberNibiruInternal.data,
    },
  }

export const adminCheck_ownerSessionTargetUserAdmin: TestData<ChatAuditInput> =
  {
    goldReq: "just tg-sai audit --chat -1002094035214 --target-user 8274304175",
    notes:
      "Target chat after add-owner admin promotion: the session user is the owner and the target user is admin, making ownership transfer eligible.",
    data: {
      chatId: chatNibiruInternalId,
      targetUserId: corinneAntoniaUserId,
      self: sessionUserUnique.data,
      actor: getChatMember_sessionOwnerNibiruInternal.data,
      target: getChatMember_ownerTargetAdminNibiruInternal.data,
    },
  }
