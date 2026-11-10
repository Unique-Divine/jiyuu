import type { Database } from "bun:sqlite"

/**
 * How far one pool chat got. A stage is saved before the call that leaves
 * it, so a crash resumes at the first step not known to be done.
 */
export type PoolStage =
  | "creating"
  | "creation_unknown"
  | "created"
  | "bot_added"
  | "promoted"
  | "photo_set"
  | "registered"

/**
 * `active`: the filler works on it. `failed`: stopped for an operator, and
 * no new pool chat is made until it is retried or retired. `done`:
 * registered with Deal Desk. `retired`: given up by an operator, or a
 * create Telegram refused for certain.
 */
export type PoolStatus = "active" | "failed" | "done" | "retired"

/** One row of `pool_fill`, in camelCase. */
export interface PoolFill {
  ref: string
  accountId: number
  stage: PoolStage
  status: PoolStatus
  chatId: number | null
  /** Bot add calls sent and not refused as a whole. */
  botAdds: number
  /** Failed attempts since the last step that went through. */
  attempts: number
  /** Deal Desk outages hit while registering the chat `available`. */
  registerAttempts: number
  lastError: string | null
  nextAttemptMs: number
  createdMs: number
  updatedMs: number
}

interface PoolRow {
  ref: string
  account_id: number
  stage: string
  status: string
  chat_id: number | null
  bot_adds: number
  attempts: number
  register_attempts: number
  last_error: string | null
  next_attempt_ms: number
  created_ms: number
  updated_ms: number
}

function fromRow(row: PoolRow): PoolFill {
  return {
    ref: row.ref,
    accountId: row.account_id,
    stage: row.stage as PoolStage,
    status: row.status as PoolStatus,
    chatId: row.chat_id,
    botAdds: row.bot_adds,
    attempts: row.attempts,
    registerAttempts: row.register_attempts,
    lastError: row.last_error,
    nextAttemptMs: row.next_attempt_ms,
    createdMs: row.created_ms,
    updatedMs: row.updated_ms,
  }
}

/** Fields `PoolJournal.update` may change. */
export type PoolUpdate = Partial<
  Pick<
    PoolFill,
    | "stage"
    | "status"
    | "chatId"
    | "botAdds"
    | "attempts"
    | "registerAttempts"
    | "lastError"
    | "nextAttemptMs"
  >
>

/**
 * The local pool fill journal in `state.db` (`pool_fill`, `pool_pace`).
 * Deal Desk owns the pool itself; this only records what this host sent to
 * Telegram for the chats it is making.
 */
export class PoolJournal {
  constructor(private db: Database) {}

  /** Starts a fill at stage `creating`. Throws when `ref` exists. */
  start(input: { ref: string; accountId: number; nowMs: number }): PoolFill {
    this.db.run(
      "INSERT INTO pool_fill (ref, account_id, stage, status, chat_id, " +
        "bot_adds, attempts, last_error, next_attempt_ms, created_ms, " +
        "updated_ms) VALUES (?, ?, 'creating', 'active', NULL, 0, 0, NULL, " +
        "?, ?, ?)",
      [input.ref, input.accountId, input.nowMs, input.nowMs, input.nowMs],
    )
    return this.get(input.ref)!
  }

  get(ref: string): PoolFill | null {
    const row = this.db
      .query<PoolRow, [string]>("SELECT * FROM pool_fill WHERE ref = ?")
      .get(ref)
    return row ? fromRow(row) : null
  }

  /**
   * The account's fill that is `active` or `failed`, oldest first. The
   * filler keeps at most one; a failed one blocks new fills.
   */
  open(accountId: number): PoolFill | null {
    const row = this.db
      .query<PoolRow, [number]>(
        "SELECT * FROM pool_fill WHERE account_id = ? " +
          "AND status IN ('active', 'failed') ORDER BY created_ms, ref LIMIT 1",
      )
      .get(accountId)
    return row ? fromRow(row) : null
  }

  /** Writes `update` to `ref`; throws when the row is missing. */
  update(ref: string, update: PoolUpdate, nowMs: number): PoolFill {
    const current = this.get(ref)
    if (!current) throw new Error("Pool fill record missing")
    const next = { ...current, ...update }
    this.db.run(
      "UPDATE pool_fill SET stage = ?, status = ?, chat_id = ?, " +
        "bot_adds = ?, attempts = ?, register_attempts = ?, last_error = ?, " +
        "next_attempt_ms = ?, updated_ms = ? WHERE ref = ?",
      [
        next.stage,
        next.status,
        next.chatId,
        next.botAdds,
        next.attempts,
        next.registerAttempts,
        next.lastError,
        next.nextAttemptMs,
        nowMs,
        ref,
      ],
    )
    return this.get(ref)!
  }

  /**
   * Whether the account's latest fill retired with `lastError` came after
   * its latest `done` fill: a create that went missing with no successful
   * fill since.
   */
  retiredSinceDone(accountId: number, lastError: string): boolean {
    const row = this.db
      .query<{ retired: number | null; done: number | null }, [
        number,
        string,
        number,
      ]>(
        "SELECT " +
          "(SELECT MAX(updated_ms) FROM pool_fill WHERE account_id = ? " +
          "AND status = 'retired' AND last_error = ?) AS retired, " +
          "(SELECT MAX(updated_ms) FROM pool_fill WHERE account_id = ? " +
          "AND status = 'done') AS done",
      )
      .get(accountId, lastError, accountId)
    if (!row || row.retired === null) return false
    return row.done === null || row.retired > row.done
  }

  /** Every fill of `accountId`, newest first, at most `limit`. */
  list(accountId: number, limit = 50): PoolFill[] {
    return this.db
      .query<PoolRow, [number, number]>(
        "SELECT * FROM pool_fill WHERE account_id = ? " +
          "ORDER BY created_ms DESC, ref LIMIT ?",
      )
      .all(accountId, limit)
      .map(fromRow)
  }

  /** Closes the database. */
  close(): void {
    try {
      this.db.close(true)
    } catch {
      // Already closed.
    }
  }

  /** Earliest time the account's next pool chat may be created. */
  nextFillMs(accountId: number): number {
    return (
      this.db
        .query<{ next_fill_ms: number }, [number]>(
          "SELECT next_fill_ms FROM pool_pace WHERE account_id = ?",
        )
        .get(accountId)?.next_fill_ms ?? 0
    )
  }

  /** Moves the account's next fill to `atMs` or later. */
  paceFill(accountId: number, atMs: number): void {
    this.db.run(
      "INSERT INTO pool_pace (account_id, next_fill_ms) VALUES (?, ?) " +
        "ON CONFLICT (account_id) DO UPDATE SET " +
        "next_fill_ms = MAX(next_fill_ms, excluded.next_fill_ms)",
      [accountId, atMs],
    )
  }
}
