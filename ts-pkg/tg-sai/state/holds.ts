import type { Database, Statement } from "bun:sqlite"

import { openStateDb } from "./db.ts"

/**
 * Holds key by action family (group), on the assumption that Telegram
 * limits related methods together. Telegram does not publish which methods
 * share a limit; these groups are our conservative guess. Background: agent
 * skill `ud-tg-telegram`, page
 * `telegram-limits.md` (boku `priv-skills/ud-tg-telegram/`).
 */
export type HoldGroup =
  | "invite"
  | "create"
  | "resolve"
  | "send"
  | `method:${string}`

export type HoldSource =
  | "flood_wait"
  | "flood_premium_wait"
  | "peer_flood"
  | "manual"

export interface ParsedHold {
  source: Exclude<HoldSource, "manual">
  waitS: number
}

/** One row of `flood_hold`, in camelCase. */
export interface FloodHold {
  accountId: string
  grp: HoldGroup
  untilMs: number
  method: string
  waitS: number | null
  source: HoldSource
  requestId: string | null
  createdMs: number
  updatedMs: number
}

export interface RecordHoldInput {
  accountId: string | number
  grp: HoldGroup
  method: string
  source: HoldSource
  /** Telegram's wait; `untilMs` defaults to `holdUntil(nowMs, waitS)`. */
  waitS?: number | null
  /** Explicit end time (e.g. a manual hold). Wins over `waitS`. */
  untilMs?: number
  requestId?: string | null
  nowMs?: number
}

export const PEER_FLOOD_WAIT_S = 24 * 60 * 60
export const MIN_HOLD_MARGIN_MS = 10_000
export const MAX_HOLD_MS = 365 * 24 * 60 * 60 * 1000

const GROUPS: Readonly<Record<string, HoldGroup>> = {
  "channels.inviteToChannel": "invite",
  "messages.addChatUser": "invite",
  "channels.createChannel": "create",
  "messages.createChat": "create",
  "contacts.resolveUsername": "resolve",
  "messages.sendMessage": "send",
  "messages.sendMedia": "send",
  "messages.sendMultiMedia": "send",
  "messages.forwardMessages": "send",
}

/** Hold group of a TL method name; unknown methods key by their own name. */
export function methodGroup(method: string): HoldGroup {
  return GROUPS[method] ?? `method:${method}`
}

/** Groups that must fail closed when hold state cannot be read. */
export function isFailClosedGroup(grp: HoldGroup): boolean {
  return grp === "invite" || grp === "create"
}

const WAIT_PATTERN = /^(FLOOD_WAIT|FLOOD_PREMIUM_WAIT)_(\d{1,10})$/

/**
 * Hold implied by a raw Telegram error message: `FLOOD_WAIT_N`,
 * `FLOOD_PREMIUM_WAIT_N`, or `PEER_FLOOD` (24 h). `null` for anything else.
 */
export function parseHoldError(
  errorMessage: string | null | undefined,
): ParsedHold | null {
  if (typeof errorMessage !== "string") return null
  if (errorMessage === "PEER_FLOOD") {
    return { source: "peer_flood", waitS: PEER_FLOOD_WAIT_S }
  }
  const match = WAIT_PATTERN.exec(errorMessage)
  if (!match) return null
  const waitS = Number(match[2])
  if (!Number.isSafeInteger(waitS)) return null
  return {
    source: match[1] === "FLOOD_WAIT" ? "flood_wait" : "flood_premium_wait",
    waitS,
  }
}

/** `nowMs + waitS` plus a margin of max(10 s, 1 % of the wait). */
export function holdUntil(nowMs: number, waitS: number): number {
  const waitMs = Math.max(0, waitS) * 1000
  return nowMs + waitMs + Math.max(MIN_HOLD_MARGIN_MS, Math.ceil(waitMs / 100))
}

/** Whole seconds left on a hold, at least 1 while it is active. */
export function remainingSeconds(untilMs: number, nowMs: number): number {
  return Math.max(1, Math.ceil((untilMs - nowMs) / 1000))
}

/** Thrown by every `HoldStore` method when the store failed to open. */
export class HoldStoreUnavailableError extends Error {
  readonly code = 500
  readonly text = "TG_SAI_HOLD_STATE_UNAVAILABLE"
  constructor(readonly reason: string) {
    super("TG_SAI_HOLD_STATE_UNAVAILABLE")
    this.name = "HoldStoreUnavailableError"
  }
}

interface HoldRow {
  account_id: string
  grp: string
  until_ms: number
  method: string
  wait_s: number | null
  source: string
  request_id: string | null
  created_ms: number
  updated_ms: number
}

function fromRow(row: HoldRow): FloodHold {
  return {
    accountId: row.account_id,
    grp: row.grp as HoldGroup,
    untilMs: row.until_ms,
    method: row.method,
    waitS: row.wait_s,
    source: row.source as HoldSource,
    requestId: row.request_id,
    createdMs: row.created_ms,
    updatedMs: row.updated_ms,
  }
}

// SET expressions read the pre-update row, so each CASE compares against the
// stored hold. A longer hold replaces the details; an expired one restarts.
const UPSERT_SQL = `INSERT INTO flood_hold (
  account_id, grp, until_ms, method, wait_s, source, request_id,
  created_ms, updated_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT (account_id, grp) DO UPDATE SET
  until_ms = MAX(until_ms, excluded.until_ms),
  method = CASE WHEN excluded.until_ms > until_ms
    THEN excluded.method ELSE method END,
  wait_s = CASE WHEN excluded.until_ms > until_ms
    THEN excluded.wait_s ELSE wait_s END,
  source = CASE WHEN excluded.until_ms > until_ms
    THEN excluded.source ELSE source END,
  request_id = CASE WHEN excluded.until_ms > until_ms
    THEN excluded.request_id ELSE request_id END,
  created_ms = CASE WHEN until_ms <= excluded.created_ms
    THEN excluded.created_ms ELSE created_ms END,
  updated_ms = excluded.updated_ms
RETURNING *`

const ACTIVE_SQL =
  "SELECT * FROM flood_hold WHERE account_id = ? AND grp = ? AND until_ms > ?"

/**
 * Durable flood holds in `state.db`. Construct with `HoldStore.open`; a
 * store that failed to open has `unavailable` set and every method throws
 * `HoldStoreUnavailableError`, so callers can fail closed.
 */
export class HoldStore {
  private upsert: Statement<HoldRow> | null = null
  private active: Statement<HoldRow> | null = null

  private constructor(
    private db: Database | null,
    private reason: string | null,
  ) {}

  /** Opens the store at `path`. Never throws. */
  static open(path: string): HoldStore {
    try {
      return new HoldStore(openStateDb(path), null)
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error)
      return new HoldStore(null, reason || "cannot open state database")
    }
  }

  /** Wraps an already-migrated database (e.g. from `openStateDb`). */
  static fromDb(db: Database): HoldStore {
    return new HoldStore(db, null)
  }

  /** A store that reports `unavailable`, for callers and tests. */
  static unavailable(reason: string): HoldStore {
    return new HoldStore(null, reason)
  }

  get unavailable(): boolean {
    return this.db === null
  }

  /** Why the store is unavailable; `null` while it is open. */
  get unavailableReason(): string | null {
    return this.db === null ? (this.reason ?? "closed") : null
  }

  private requireDb(): Database {
    if (!this.db) {
      throw new HoldStoreUnavailableError(this.reason ?? "closed")
    }
    return this.db
  }

  /**
   * Upserts a hold, keeping the later `until_ms` of the stored and new
   * holds. The end time is capped at one year from `nowMs`. Returns the
   * stored row.
   */
  recordHold(input: RecordHoldInput): FloodHold {
    const db = this.requireDb()
    const nowMs = input.nowMs ?? Date.now()
    const waitS = input.waitS ?? null
    let untilMs =
      input.untilMs ?? (waitS === null ? NaN : holdUntil(nowMs, waitS))
    if (!Number.isFinite(untilMs)) {
      throw new Error("recordHold needs waitS or untilMs")
    }
    untilMs = Math.min(Math.round(untilMs), nowMs + MAX_HOLD_MS)
    this.upsert ??= db.prepare<HoldRow, (string | number | null)[]>(UPSERT_SQL)
    const row = this.upsert.get(
      String(input.accountId),
      input.grp,
      untilMs,
      input.method,
      waitS,
      input.source,
      input.requestId ?? null,
      nowMs,
      nowMs,
    )
    if (!row) throw new Error("flood_hold upsert returned no row")
    return fromRow(row)
  }

  /** The hold on `grp` for `accountId` if it ends after `nowMs`. */
  activeHold(
    accountId: string | number,
    grp: HoldGroup,
    nowMs: number = Date.now(),
  ): FloodHold | null {
    const db = this.requireDb()
    this.active ??= db.prepare<HoldRow, (string | number)[]>(ACTIVE_SQL)
    const row = this.active.get(String(accountId), grp, nowMs)
    return row ? fromRow(row) : null
  }

  /** Active holds, latest end first; all accounts when `accountId` is unset. */
  activeHolds(
    accountId?: string | number | null,
    nowMs: number = Date.now(),
  ): FloodHold[] {
    const db = this.requireDb()
    const rows =
      accountId === undefined || accountId === null
        ? db
            .query<HoldRow, [number]>(
              "SELECT * FROM flood_hold WHERE until_ms > ? " +
                "ORDER BY until_ms DESC, account_id, grp",
            )
            .all(nowMs)
        : db
            .query<HoldRow, [string, number]>(
              "SELECT * FROM flood_hold WHERE account_id = ? AND until_ms > ? " +
                "ORDER BY until_ms DESC, grp",
            )
            .all(String(accountId), nowMs)
    return rows.map(fromRow)
  }

  /** Operator override: deletes a hold. Returns whether one existed. */
  clearHold(accountId: string | number, grp: HoldGroup): boolean {
    const db = this.requireDb()
    const result = db.run(
      "DELETE FROM flood_hold WHERE account_id = ? AND grp = ?",
      [String(accountId), grp],
    )
    return result.changes > 0
  }

  /** Whether `close` was called (as opposed to a failed open). */
  get closed(): boolean {
    return this.db === null && this.reason === "closed"
  }

  /** Closes the database; the store then reports `unavailable`. */
  close(): void {
    const db = this.db
    this.db = null
    this.reason ??= "closed"
    try {
      this.upsert?.finalize()
      this.active?.finalize()
      db?.close(true)
    } catch {
      // Already closed.
    }
    this.upsert = null
    this.active = null
  }
}

/** Least time between attempts to reopen a store that failed to open. */
export const HOLD_STORE_REOPEN_MS = 30_000

export interface ReopeningHoldStoreOptions {
  /** Opens the store; default `HoldStore.open(path)`. */
  open?: (path: string) => HoldStore
  now?: () => number
  /** Default `HOLD_STORE_REOPEN_MS`. */
  reopenMs?: number
}

/**
 * Hands out one open `HoldStore` for `path`. An open that fails is not
 * kept: the unavailable store is returned (so callers fail closed) and the
 * next call at least `reopenMs` later tries again. A store closed by its
 * user is reopened on the next call.
 */
export class ReopeningHoldStore {
  private store: HoldStore | null = null
  private attemptMs = -Infinity
  private readonly openStore: (path: string) => HoldStore
  private readonly now: () => number
  private readonly reopenMs: number

  constructor(
    readonly path: string,
    options: ReopeningHoldStoreOptions = {},
  ) {
    this.openStore = options.open ?? ((p) => HoldStore.open(p))
    this.now = options.now ?? Date.now
    this.reopenMs = options.reopenMs ?? HOLD_STORE_REOPEN_MS
  }

  /** The open store, or the last unavailable one inside the backoff. */
  get(nowMs: number = this.now()): HoldStore {
    const current = this.store
    if (current && !current.unavailable) return current
    if (
      current &&
      !current.closed &&
      nowMs - this.attemptMs < this.reopenMs
    ) {
      return current
    }
    this.attemptMs = nowMs
    let next: HoldStore
    try {
      next = this.openStore(this.path)
    } catch (error) {
      next = HoldStore.unavailable(
        error instanceof Error ? error.message : String(error),
      )
    }
    this.store = next
    return next
  }

  /** Closes the store; the next `get` reopens it. */
  close(): void {
    this.store?.close()
    this.store = null
    this.attemptMs = -Infinity
  }
}
