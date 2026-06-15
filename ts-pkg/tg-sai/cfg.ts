export interface TelegramIdentity {
  id: number
  handle: string
  displayName: string
  kind: "user" | "bot"
}

export interface TelegramChatIdentity {
  id: number
  title: string
  kind: "channel" | "group"
}

/**
 * Telegram bot identity for the Sai team bot. The bot is the automation target
 * to add and promote in candidate chats.
 */
export const saiBot = {
  id: 8983431101,
  handle: "SaiTeam_bot",
  displayName: "Sai Team",
  kind: "bot",
} satisfies TelegramIdentity

/**
 * Known Telegram user identities used by tg-sai operator workflows.
 */
export const USERS = {
  /**
   * Default operational target user to promote to admin or prepare for
   * ownership transfer.
   */
  uniqueDivine: {
    id: 1912700300,
    handle: "uniquedivine",
    displayName: "Unique (NIBIRU)",
    kind: "user",
  },

  /**
   * Corinne's primary account. This account owns or administers some candidate
   * chats used by tg-sai operator flows.
   */
  corinne: {
    id: 7930936303,
    handle: "corinnebernett",
    displayName: "Corinne Bernett",
    kind: "user",
  },

  /**
   * Corinne's alternate account. Use this account as a controlled
   * owner-transfer target when dry-running or testing against the internal
   * Nibiru chat before touching partner or production chats.
   */
  corinneAlt: {
    id: 8274304175,
    handle: "corinneantonia",
    displayName: "Corinne Antonia",
    kind: "user",
  },
} satisfies Record<string, TelegramIdentity>

/**
 * Internal Nibiru chat used as a controlled target for tg-sai dry runs and
 * ownership-transfer tests.
 */
export const nibiruInternal = {
  id: -1002094035214,
  title: "Nibiru | Internal",
  kind: "channel",
} satisfies TelegramChatIdentity
