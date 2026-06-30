export interface TelegramIdentity {
  id: number
  handle: string
  displayName: string
  kind: "user" | "bot"
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
 * Telegram user identity for Unique (`uniquedivine`). In this workflow, Unique
 * is the target user to promote to admin or prepare for ownership transfer.
 */
export const uniqueDivine = {
  id: 1912700300,
  handle: "uniquedivine",
  displayName: "Unique (NIBIRU)",
  kind: "user",
} satisfies TelegramIdentity

