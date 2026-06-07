/**
 * One chat entry consumed by command `tg-sai ops sai-bootstrap`.
 *
 * Inputs can come from a plain text file with one chat ID per line or from JSONL
 * records produced by command `tg-sai ops sai-find`.
 */
export interface ChatInput {
  /** Telegram chat ID or username-like peer accepted by mtcute calls. */
  chatId: number | string
  /** Human-readable chat title from the input file, when present. */
  title: string | null
  /** Original input line for diagnostics and operator review. */
  raw: string
}

/**
 * JSONL record emitted by command `tg-sai ops sai-find`.
 */
export interface SaiChatRecord {
  /** Telegram chat ID visible to the connected user session. */
  chatId: number | string
  /** Chat title used for operator review and Sai-title filtering. */
  title: string
  /** mtcute chat type, such as `group`, `supergroup`, or `channel`. */
  type: string
  /** Public username when Telegram exposes one for the chat. */
  username: string | null
}

/**
 * Returns true when a dialog title looks like a Sai operations chat.
 *
 * The filter intentionally avoids matching words such as `Kudasai`; discovery
 * should produce a review file, not an unreviewed execution set.
 */
export function isSaiChatTitle(title: string): boolean {
  return /\bsai\b|sai\.fun|sai<>|sai\s*<>/i.test(title)
}

/**
 * Parses one chat input line.
 *
 * Supported formats:
 *
 * - `<chat-id>`
 * - `<chat-id> Sai <> Example`
 * - `{"chatId":"<chat-id>","title":"Sai <> Example"}`
 * - `{"chat_id":"<chat-id>","title":"Sai <> Example"}`
 *
 * Empty lines and comment lines beginning with `#` are ignored.
 */
export function parseChatInputLine(line: string): ChatInput | null {
  const trimmed = line.trim()
  if (!trimmed || trimmed.startsWith("#")) {
    return null
  }

  if (trimmed.startsWith("{")) {
    const parsed = JSON.parse(trimmed) as {
      chatId?: number | string
      chat_id?: number | string
      title?: string
    }
    const chatId = parsed.chatId ?? parsed.chat_id
    if (chatId === undefined) {
      throw new Error(`Missing chatId in JSONL input line: ${trimmed}`)
    }
    return {
      chatId,
      title: parsed.title ?? null,
      raw: line,
    }
  }

  const [firstToken, ...titleTokens] = trimmed.split(/\s+/)
  if (!firstToken) {
    return null
  }
  const chatIdToken = firstToken.replace(/`/g, "")
  const asNumber = Number(chatIdToken)
  const chatId = Number.isFinite(asNumber) ? asNumber : chatIdToken

  return {
    chatId,
    title: titleTokens.length > 0 ? titleTokens.join(" ") : null,
    raw: line,
  }
}

/**
 * Parses a full chat input file into ordered chat entries.
 */
export function parseChatInputFile(contents: string): ChatInput[] {
  return contents
    .split(/\r?\n/)
    .map(parseChatInputLine)
    .filter((chat): chat is ChatInput => chat !== null)
}

/**
 * Serializes a discovered Sai chat as one JSONL line.
 */
export function formatSaiChatJsonLine(record: SaiChatRecord): string {
  return JSON.stringify(record)
}
