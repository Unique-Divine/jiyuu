/**
 * Read-only queries over the telemetry `event` table. Every function takes an
 * open `bun:sqlite` Database and plain filter values so tests can run them
 * against a temp database populated with real rows.
 */
import { Database } from "bun:sqlite"

import {
  ATTEMPT_EVENT,
  CALL_EVENT,
  FLOOD_WAIT_EVENT,
  RUN_END_EVENT,
  RUN_START_EVENT,
  WORKER_TICK_EVENT,
} from "./db"
import type { EventRow } from "./db"

/** Outcomes that count as failures. */
export const ERROR_OUTCOMES = ["error", "timeout", "aborted"] as const

const ERROR_OUTCOMES_SQL = ERROR_OUTCOMES.map((o) => `'${o}'`).join(", ")

type SqlParam = string | number | null

export interface EventFilter {
  /** Lower bound on `ts_ms`, inclusive. */
  sinceMs?: number
  accountId?: number
  method?: string
  runId?: string
  /** Only failed rows and flood waits. */
  errorsOnly?: boolean
}

interface WhereParts {
  clauses: string[]
  params: SqlParam[]
}

/** Builds `AND`-joined filter clauses on table alias `e`. */
export function filterClauses(filter: EventFilter): WhereParts {
  const clauses: string[] = []
  const params: SqlParam[] = []
  if (filter.sinceMs !== undefined) {
    clauses.push("e.ts_ms >= ?")
    params.push(filter.sinceMs)
  }
  if (filter.accountId !== undefined) {
    clauses.push("e.account_id = ?")
    params.push(filter.accountId)
  }
  if (filter.method !== undefined) {
    clauses.push("e.method = ?")
    params.push(filter.method)
  }
  if (filter.runId !== undefined) {
    clauses.push("e.run_id = ?")
    params.push(filter.runId)
  }
  if (filter.errorsOnly) {
    clauses.push(
      `(e.outcome IN (${ERROR_OUTCOMES_SQL}) ` +
        `OR e.event = '${FLOOD_WAIT_EVENT}')`,
    )
  }
  return { clauses, params }
}

function andAll(clauses: string[]): string {
  return clauses.length === 0 ? "" : ` AND ${clauses.join(" AND ")}`
}

/** True when a completion row exists for call row `e`. */
const HAS_COMPLETION_SQL = `EXISTS (
  SELECT 1 FROM event c
  WHERE c.request_id = e.request_id
    AND c.event = '${CALL_EVENT}'
    AND c.outcome != 'started'
)`

export interface TailOptions extends EventFilter {
  limit: number
}

/**
 * Newest-first call, attempt, and flood-wait rows. A call appears once: as
 * its completion row, or as its `started` row while no completion exists.
 */
export function tailEvents(db: Database, options: TailOptions): EventRow[] {
  const { clauses, params } = filterClauses(options)
  const sql = `SELECT e.* FROM event e
WHERE e.event IN ('${CALL_EVENT}', '${ATTEMPT_EVENT}', '${FLOOD_WAIT_EVENT}')
  AND NOT (e.event = '${CALL_EVENT}' AND e.outcome = 'started'
    AND ${HAS_COMPLETION_SQL})${andAll(clauses)}
ORDER BY e.ts_ms DESC, e.event_id DESC
LIMIT ?`
  return db.query<EventRow, SqlParam[]>(sql).all(...params, options.limit)
}

/**
 * Chronological rows sharing `id` as request, run, job, or event id.
 */
export function showTimeline(db: Database, id: string): EventRow[] {
  return db
    .query<EventRow, SqlParam[]>(
      `SELECT * FROM event
WHERE request_id = ? OR run_id = ? OR job_id = ? OR event_id = ?
ORDER BY ts_ms ASC, event_id ASC`,
    )
    .all(id, id, id, id)
}

/** `--by` names accepted by `countEvents`, mapped to columns. */
export const COUNT_COLUMNS: Readonly<Record<string, string>> = {
  method: "method",
  outcome: "outcome",
  account: "account_id",
  account_id: "account_id",
  event: "event",
  subsystem: "subsystem",
  command: "command",
  profile: "profile",
  error_code: "error_code",
  err: "err",
  run: "run_id",
  run_id: "run_id",
}

export type CountLayer = "attempt" | "call"

export interface CountOptions {
  sinceMs?: number
  accountId?: number
  by: string[]
  layer: CountLayer
}

export type CountRow = Record<string, string | number | null> & {
  count: number
}

/**
 * Parses a comma-separated `--by` list into column names. Throws on an
 * unknown name or an empty list.
 */
export function parseCountBy(value: string): string[] {
  const names = value
    .split(",")
    .map((name) => name.trim())
    .filter((name) => name.length > 0)
  if (names.length === 0) throw new Error("--by needs at least one field")
  const columns = names.map((name) => {
    const column = COUNT_COLUMNS[name]
    if (!column) {
      throw new Error(
        `Unknown --by field "${name}"; use one of: ` +
          Object.keys(COUNT_COLUMNS).join(", "),
      )
    }
    return column
  })
  return [...new Set(columns)]
}

/**
 * Row counts grouped by `by` columns. The attempt layer counts requests sent
 * to Telegram; the call layer counts finished logical calls.
 */
export function countEvents(db: Database, options: CountOptions): CountRow[] {
  const known = new Set(Object.values(COUNT_COLUMNS))
  for (const column of options.by) {
    if (!known.has(column)) throw new Error(`Unknown count column: ${column}`)
  }
  if (options.by.length === 0) throw new Error("count needs a group column")
  const { clauses, params } = filterClauses({
    sinceMs: options.sinceMs,
    accountId: options.accountId,
  })
  const layer =
    options.layer === "call"
      ? `e.event = '${CALL_EVENT}' AND e.outcome != 'started'`
      : `e.event = '${ATTEMPT_EVENT}'`
  const cols = options.by.map((c) => `e.${c} AS ${c}`).join(", ")
  const group = options.by.map((c) => `e.${c}`).join(", ")
  const sql = `SELECT ${cols}, COUNT(*) AS count FROM event e
WHERE ${layer}${andAll(clauses)}
GROUP BY ${group}
ORDER BY count DESC, ${group}`
  return db.query<CountRow, SqlParam[]>(sql).all(...params)
}

export interface FloodSummaryRow {
  event: string
  account_id: number | null
  method: string | null
  wait_s: number
  count: number
  latest_ms: number
}

export interface ActiveFloodRow {
  account_id: number | null
  method: string | null
  wait_s: number
  started_ms: number
  ends_ms: number
  request_id: string | null
}

export interface FloodReport {
  floods: FloodSummaryRow[]
  active: ActiveFloodRow[]
}

export interface FloodOptions {
  sinceMs?: number
  accountId?: number
  nowMs: number
}

/** Flood waits: mtcute sleeps and attempts that returned a wait. */
const FLOOD_ROWS_SQL = `(e.event = '${FLOOD_WAIT_EVENT}'
  OR (e.event = '${ATTEMPT_EVENT}' AND e.wait_s IS NOT NULL))`

/**
 * Flood waits grouped by layer, account, method, and wait, plus each
 * account/method pair whose latest wait has not ended by `nowMs`. Active
 * waits ignore `sinceMs` so a long wait that began earlier still shows.
 */
export function floodReport(db: Database, options: FloodOptions): FloodReport {
  const summary = filterClauses({
    sinceMs: options.sinceMs,
    accountId: options.accountId,
  })
  const floods = db
    .query<FloodSummaryRow, SqlParam[]>(
      `SELECT e.event AS event, e.account_id AS account_id,
  e.method AS method, e.wait_s AS wait_s, COUNT(*) AS count,
  MAX(e.ts_ms) AS latest_ms
FROM event e
WHERE ${FLOOD_ROWS_SQL} AND e.wait_s IS NOT NULL${andAll(summary.clauses)}
GROUP BY e.event, e.account_id, e.method, e.wait_s
ORDER BY latest_ms DESC`,
    )
    .all(...summary.params)
  const activeFilter = filterClauses({ accountId: options.accountId })
  // SQLite takes bare columns from the row that supplied MAX().
  const active = db
    .query<ActiveFloodRow, SqlParam[]>(
      `SELECT e.account_id AS account_id, e.method AS method,
  e.wait_s AS wait_s, e.ts_ms AS started_ms,
  MAX(e.ts_ms + e.wait_s * 1000) AS ends_ms, e.request_id AS request_id
FROM event e
WHERE ${FLOOD_ROWS_SQL} AND e.wait_s IS NOT NULL${andAll(activeFilter.clauses)}
GROUP BY e.account_id, e.method
HAVING ends_ms > ?
ORDER BY ends_ms DESC`,
    )
    .all(...activeFilter.params, options.nowMs)
  return { floods, active }
}

/**
 * `tg.rpc.call` rows still `started` with no completion row for their
 * `request_id`: the caller cannot know whether Telegram applied them.
 */
export function uncertainCalls(
  db: Database,
  options: { sinceMs?: number } = {},
): EventRow[] {
  const { clauses, params } = filterClauses({ sinceMs: options.sinceMs })
  return db
    .query<EventRow, SqlParam[]>(
      `SELECT e.* FROM event e
WHERE e.event = '${CALL_EVENT}' AND e.outcome = 'started'
  AND NOT ${HAS_COMPLETION_SQL}${andAll(clauses)}
ORDER BY e.ts_ms DESC, e.event_id DESC`,
    )
    .all(...params)
}

export interface RunRow {
  run_id: string
  started_ms: number
  command: string | null
  subsystem: string
  account_id: number | null
  profile: string | null
  host: string | null
  pid: number | null
  ended_ms: number | null
  /** `run.end` outcome, or null when the run has no end row. */
  outcome: string | null
  error_code: number | null
  err: string | null
  duration_ms: number | null
  calls: number
  errors: number
  attempts: number
  ticks: number
}

/**
 * Newest `run.start` rows joined to their latest `run.end`, with call,
 * failed-call, attempt, and worker-tick counts. Duration falls back to the
 * gap between the two rows when `run.end` lacks `duration_ms`.
 */
export function recentRuns(db: Database, limit: number): RunRow[] {
  return db
    .query<RunRow, SqlParam[]>(
      `SELECT s.run_id AS run_id, s.ts_ms AS started_ms, s.command AS command,
  s.subsystem AS subsystem, s.account_id AS account_id, s.profile AS profile,
  s.host AS host, s.pid AS pid, en.ts_ms AS ended_ms, en.outcome AS outcome,
  en.error_code AS error_code, en.err AS err,
  COALESCE(en.duration_ms, en.ts_ms - s.ts_ms) AS duration_ms,
  (SELECT COUNT(*) FROM event x WHERE x.run_id = s.run_id
    AND x.event = '${CALL_EVENT}' AND x.outcome != 'started') AS calls,
  (SELECT COUNT(*) FROM event x WHERE x.run_id = s.run_id
    AND x.event = '${CALL_EVENT}'
    AND x.outcome IN (${ERROR_OUTCOMES_SQL})) AS errors,
  (SELECT COUNT(*) FROM event x WHERE x.run_id = s.run_id
    AND x.event = '${ATTEMPT_EVENT}') AS attempts,
  (SELECT COUNT(*) FROM event x WHERE x.run_id = s.run_id
    AND x.event = '${WORKER_TICK_EVENT}') AS ticks
FROM event s
LEFT JOIN event en ON en.event_id = (
  SELECT y.event_id FROM event y
  WHERE y.run_id = s.run_id AND y.event = '${RUN_END_EVENT}'
  ORDER BY y.ts_ms DESC, y.event_id DESC LIMIT 1
)
WHERE s.event = '${RUN_START_EVENT}'
ORDER BY s.ts_ms DESC, s.event_id DESC
LIMIT ?`,
    )
    .all(limit)
}

export interface KindCount {
  event: string
  rows: number
}

export interface EventStats {
  rows: number
  oldest_ms: number | null
  newest_ms: number | null
  kinds: KindCount[]
}

export function eventStats(db: Database): EventStats {
  const totals = db
    .query<
      { rows: number; oldest_ms: number | null; newest_ms: number | null },
      []
    >(
      "SELECT COUNT(*) AS rows, MIN(ts_ms) AS oldest_ms, " +
        "MAX(ts_ms) AS newest_ms FROM event",
    )
    .get()
  const kinds = db
    .query<KindCount, []>(
      "SELECT event, COUNT(*) AS rows FROM event GROUP BY event " +
        "ORDER BY rows DESC, event",
    )
    .all()
  return {
    rows: totals?.rows ?? 0,
    oldest_ms: totals?.oldest_ms ?? null,
    newest_ms: totals?.newest_ms ?? null,
    kinds,
  }
}

/** Oldest-first rows since `sinceMs`, streamed. */
export function* exportEvents(
  db: Database,
  options: { sinceMs?: number } = {},
): Generator<EventRow> {
  const { clauses, params } = filterClauses({ sinceMs: options.sinceMs })
  const where = clauses.length === 0 ? "" : ` WHERE ${clauses.join(" AND ")}`
  const statement = db.query<EventRow, SqlParam[]>(
    `SELECT e.* FROM event e${where} ORDER BY e.ts_ms ASC, e.event_id ASC`,
  )
  for (const row of statement.iterate(...params)) yield row
}

/**
 * Opens an existing telemetry database read-only: no create, no pragmas
 * that write, no migrations.
 */
export function openTelemetryReadOnly(path: string): Database {
  const db = new Database(path, { readonly: true, strict: true })
  try {
    db.run("PRAGMA busy_timeout = 5000")
    return db
  } catch (error) {
    db.close()
    throw error
  }
}
