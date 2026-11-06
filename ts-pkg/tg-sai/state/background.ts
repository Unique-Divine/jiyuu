import type { Database } from "bun:sqlite"

import type { ProfileStoreEnv } from "../profiles.ts"

/**
 * States of a `background_add` row. `privacy` is a privacy refusal: the
 * member joins through the receipt's shared link and costs no daily cap.
 */
export type BackgroundState = "pending" | "added" | "privacy" | "failed"

/** One row of `background_add`, in camelCase. */
export interface BackgroundMember {
  requestId: string
  chatId: number
  userId: number
  role: string
  notBeforeMs: number
  attempts: number
  state: BackgroundState
  lastError: string | null
  createdMs: number
  updatedMs: number
}

/** Pacing for background adds; see `backgroundPacing`. */
export interface BackgroundPacing {
  /** Base gap between two adds. */
  gapMs: number
  /** Most adds (finished rows) per UTC day. */
  dailyMax: number
  /** Fraction of `gapMs` each gap may vary by, either way. */
  jitter: number
}

export const BACKGROUND_GAP_ENV = "TG_SAI_BACKGROUND_GAP_MS"
export const BACKGROUND_DAILY_MAX_ENV = "TG_SAI_BACKGROUND_DAILY_MAX"
export const DEFAULT_BACKGROUND_GAP_MS = 20 * 60 * 1000
export const DEFAULT_BACKGROUND_DAILY_MAX = 6
export const BACKGROUND_JITTER = 0.3
/** Attempts after which an unexplained error marks a row failed. */
export const MAX_BACKGROUND_ATTEMPTS = 5
const DAY_MS = 24 * 60 * 60 * 1000

function envInt(
  value: string | undefined,
  fallback: number,
  min: number,
): number {
  if (value === undefined || !value.trim()) return fallback
  const parsed = Number(value.trim())
  return Number.isSafeInteger(parsed) && parsed >= min ? parsed : fallback
}

/**
 * Pacing from `TG_SAI_BACKGROUND_GAP_MS` (default 20 min, at least 1000)
 * and `TG_SAI_BACKGROUND_DAILY_MAX` (default 6, 0 stops background adds).
 * Invalid values fall back to the defaults.
 */
export function backgroundPacing(
  env: ProfileStoreEnv = process.env,
): BackgroundPacing {
  return {
    gapMs: envInt(env[BACKGROUND_GAP_ENV], DEFAULT_BACKGROUND_GAP_MS, 1000),
    dailyMax: envInt(
      env[BACKGROUND_DAILY_MAX_ENV],
      DEFAULT_BACKGROUND_DAILY_MAX,
      0,
    ),
    jitter: BACKGROUND_JITTER,
  }
}

/** `gapMs` varied by up to ±`jitter`; `random` returns [0, 1). */
export function jitteredGapMs(
  pacing: Pick<BackgroundPacing, "gapMs" | "jitter">,
  random: () => number = Math.random,
): number {
  const r = Math.min(Math.max(random(), 0), 1)
  return Math.round(pacing.gapMs * (1 - pacing.jitter + 2 * pacing.jitter * r))
}

/** Start of the UTC day that contains `nowMs`. */
export function utcDayStartMs(nowMs: number): number {
  return Math.floor(nowMs / DAY_MS) * DAY_MS
}

/** Start of the next UTC day after `nowMs`. */
export function nextUtcDayMs(nowMs: number): number {
  return utcDayStartMs(nowMs) + DAY_MS
}

interface BackgroundRow {
  request_id: string
  chat_id: number
  user_id: number
  role: string
  not_before_ms: number
  attempts: number
  state: string
  last_error: string | null
  created_ms: number
  updated_ms: number
}

function fromRow(row: BackgroundRow): BackgroundMember {
  return {
    requestId: row.request_id,
    chatId: row.chat_id,
    userId: row.user_id,
    role: row.role,
    notBeforeMs: row.not_before_ms,
    attempts: row.attempts,
    state: row.state as BackgroundState,
    lastError: row.last_error,
    createdMs: row.created_ms,
    updatedMs: row.updated_ms,
  }
}

/**
 * Rows that count toward the daily cap: their `updated_ms` is when the add
 * call finished. A privacy refusal invited nobody, so it does not count.
 */
const FINISHED = "state IN ('added', 'failed')"

/**
 * The `background_add` queue in `state.db`. Rows start `pending` and end
 * `added`, `privacy` (refused; the shared link covers it), or `failed`.
 */
export class BackgroundQueue {
  constructor(private db: Database) {}

  /**
   * Queues `userIds` for `chatId`. Rows that exist keep their state, so a
   * resumed job never re-queues a finished member. Returns rows inserted.
   */
  enqueue(input: {
    requestId: string
    chatId: number
    userIds: readonly number[]
    role?: string
    notBeforeMs: number
    nowMs?: number
  }): number {
    const nowMs = input.nowMs ?? Date.now()
    const insert = this.db.prepare(
      "INSERT OR IGNORE INTO background_add (request_id, chat_id, user_id, " +
        "role, not_before_ms, attempts, state, last_error, created_ms, " +
        "updated_ms) VALUES (?, ?, ?, ?, ?, 0, 'pending', NULL, ?, ?)",
    )
    let inserted = 0
    this.db.transaction(() => {
      for (const userId of input.userIds)
        inserted += insert.run(
          input.requestId,
          input.chatId,
          userId,
          input.role ?? "member",
          input.notBeforeMs,
          nowMs,
          nowMs,
        ).changes
    })()
    return inserted
  }

  /** The pending row due first at `nowMs`, or `null`. */
  next(nowMs: number): BackgroundMember | null {
    const row = this.db
      .query<BackgroundRow, [number]>(
        "SELECT * FROM background_add WHERE state = 'pending' " +
          "AND not_before_ms <= ? " +
          "ORDER BY not_before_ms, created_ms, chat_id, user_id LIMIT 1",
      )
      .get(nowMs)
    return row ? fromRow(row) : null
  }

  /** Ends a row after an add call. */
  finish(
    chatId: number,
    userId: number,
    state: Exclude<BackgroundState, "pending">,
    error: string | null,
    nowMs: number,
  ): void {
    this.db.run(
      "UPDATE background_add SET state = ?, last_error = ?, " +
        "attempts = attempts + 1, updated_ms = ? " +
        "WHERE chat_id = ? AND user_id = ?",
      [state, error, nowMs, chatId, userId],
    )
  }

  /**
   * Keeps a row pending until `notBeforeMs`. Counts one attempt unless
   * `countAttempt` is false (the add call never ran or was not at fault).
   */
  defer(
    chatId: number,
    userId: number,
    notBeforeMs: number,
    error: string | null,
    nowMs: number,
    countAttempt = true,
  ): void {
    this.db.run(
      "UPDATE background_add SET not_before_ms = ?, last_error = ?, " +
        "attempts = attempts + ?, updated_ms = ? " +
        "WHERE chat_id = ? AND user_id = ? AND state = 'pending'",
      [notBeforeMs, error, countAttempt ? 1 : 0, nowMs, chatId, userId],
    )
  }

  /** Moves every pending row to `notBeforeMs` or later. */
  pace(notBeforeMs: number): void {
    this.db.run(
      "UPDATE background_add SET not_before_ms = MAX(not_before_ms, ?) " +
        "WHERE state = 'pending'",
      [notBeforeMs],
    )
  }

  /** Rows that finished an add call since the UTC day began. */
  addsToday(nowMs: number): number {
    return (
      this.db
        .query<{ n: number }, [number]>(
          `SELECT count(*) AS n FROM background_add WHERE ${FINISHED} ` +
            "AND updated_ms >= ?",
        )
        .get(utcDayStartMs(nowMs))?.n ?? 0
    )
  }

  /** Pending rows and the earliest `not_before_ms` among them. */
  summary(): { pending: number; nextAtMs: number | null } {
    const row = this.db
      .query<{ n: number; next: number | null }, []>(
        "SELECT count(*) AS n, MIN(not_before_ms) AS next " +
          "FROM background_add WHERE state = 'pending'",
      )
      .get()
    return { pending: row?.n ?? 0, nextAtMs: row?.next ?? null }
  }

  /** Closes the database. */
  close(): void {
    try {
      this.db.close(true)
    } catch {
      // Already closed.
    }
  }

  /** Every row for `chatId`, by user. */
  forChat(chatId: number): BackgroundMember[] {
    return this.db
      .query<BackgroundRow, [number]>(
        "SELECT * FROM background_add WHERE chat_id = ? ORDER BY user_id",
      )
      .all(chatId)
      .map(fromRow)
  }
}
