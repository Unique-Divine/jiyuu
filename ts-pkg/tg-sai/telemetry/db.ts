import { Database, type Statement } from "bun:sqlite"
import { createHash } from "node:crypto"
import { chmodSync, mkdirSync } from "node:fs"
import { hostname } from "node:os"
import { dirname, join } from "node:path"

import { configDir, type ProfileStoreEnv } from "../profiles"
// Embedded at bundle/run time so the published package carries the schema.
import eventMigrationSql from "./migrations/0001-event.sql" with {
  type: "text",
}
import eventJobIndexSql from "./migrations/0002-event-job-index.sql" with {
  type: "text",
}

export const TELEMETRY_DB_FILE = "telemetry.db"
export const TELEMETRY_ENV = "TG_SAI_TELEMETRY"

/** Values of the `event` column. */
export const CALL_EVENT = "tg.rpc.call"
export const ATTEMPT_EVENT = "tg.rpc.attempt"
export const FLOOD_WAIT_EVENT = "tg.flood.wait"
export const RUN_START_EVENT = "run.start"
export const RUN_END_EVENT = "run.end"
export const WORKER_TICK_EVENT = "worker.tick"

export type TelemetryOutcome =
  | "started"
  | "ok"
  | "error"
  | "timeout"
  | "aborted"

export type TelemetrySubsystem = "worker" | "health" | "cli"

/**
 * One telemetry row. `req` and `res` must already be summaries (see
 * `summarize.ts`); the sink only serializes them.
 */
export interface TelemetryEvent {
  eventId?: string
  requestId?: string | null
  runId: string
  jobId?: string | null
  tsMs?: number
  durationMs?: number | null
  event: string
  subsystem: TelemetrySubsystem | string
  command?: string | null
  accountId?: number | null
  profile?: string | null
  method?: string | null
  attempt?: number | null
  outcome: TelemetryOutcome
  errorCode?: number | null
  err?: string | null
  waitS?: number | null
  req?: unknown
  res?: unknown
  host?: string | null
  pid?: number | null
  version?: string | null
}

/** Row shape as stored in the `event` table. */
export interface EventRow {
  event_id: string
  request_id: string | null
  run_id: string
  job_id: string | null
  ts_ms: number
  duration_ms: number | null
  event: string
  subsystem: string
  command: string | null
  account_id: number | null
  profile: string | null
  method: string | null
  attempt: number | null
  outcome: TelemetryOutcome
  error_code: number | null
  err: string | null
  wait_s: number | null
  req: string | null
  res: string | null
  host: string | null
  pid: number | null
  version: string | null
}

export interface Migration {
  filename: string
  sql: string
}

export interface MigrationResult {
  applied: string[]
  pending: string[]
}

export const MIGRATIONS: readonly Migration[] = [
  { filename: "0001-event.sql", sql: eventMigrationSql },
  { filename: "0002-event-job-index.sql", sql: eventJobIndexSql },
]

const HISTORY_SQL = `CREATE TABLE IF NOT EXISTS schema_migrations (
  filename TEXT PRIMARY KEY,
  checksum TEXT NOT NULL,
  applied_at INTEGER NOT NULL
)`

export function migrationChecksum(sql: string): string {
  return createHash("sha256").update(sql).digest("hex")
}

export function telemetryDbPath(env: ProfileStoreEnv = process.env): string {
  return join(configDir(env), TELEMETRY_DB_FILE)
}

export function telemetryDisabled(
  env: ProfileStoreEnv = process.env,
): boolean {
  return env[TELEMETRY_ENV]?.trim().toLowerCase() === "off"
}

/**
 * Returns a time-ordered UUID (RFC 9562 v7). Uses `Bun.randomUUIDv7` when
 * the runtime provides it.
 */
export function uuidv7(nowMs: number = Date.now()): string {
  const bun = (globalThis as { Bun?: { randomUUIDv7?: unknown } }).Bun
  if (typeof bun?.randomUUIDv7 === "function") {
    return (bun.randomUUIDv7 as (encoding: "hex", ts: number) => string)(
      "hex",
      nowMs,
    )
  }
  const bytes = new Uint8Array(16)
  crypto.getRandomValues(bytes)
  let ts = Math.floor(nowMs)
  for (let i = 5; i >= 0; i -= 1) {
    bytes[i] = ts % 256
    ts = Math.floor(ts / 256)
  }
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x70
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")
  return [
    hex.slice(0, 8),
    hex.slice(8, 12),
    hex.slice(12, 16),
    hex.slice(16, 20),
    hex.slice(20),
  ].join("-")
}

/**
 * Applies pending migrations under `BEGIN IMMEDIATE`. Refuses an applied
 * migration whose checksum changed, an unknown applied migration, or a
 * history that is not a prefix of `migrations`.
 */
export function applyMigrations(
  db: Database,
  migrations: readonly Migration[] = MIGRATIONS,
  nowMs: number = Date.now(),
): MigrationResult {
  db.run("BEGIN IMMEDIATE")
  try {
    db.run(HISTORY_SQL)
    const rows = db
      .query<{ filename: string; checksum: string }, []>(
        "SELECT filename, checksum FROM schema_migrations ORDER BY filename",
      )
      .all()
    rows.forEach((row, i) => {
      const expected = migrations[i]
      if (!expected || expected.filename !== row.filename) {
        throw new Error(`Unknown or out-of-order migration: ${row.filename}`)
      }
      if (migrationChecksum(expected.sql) !== row.checksum) {
        throw new Error(`Migration checksum mismatch: ${row.filename}`)
      }
    })
    const applied = rows.map((row) => row.filename)
    const pending = migrations.slice(rows.length)
    const insert = db.prepare(
      "INSERT INTO schema_migrations (filename, checksum, applied_at) " +
        "VALUES (?, ?, ?)",
    )
    for (const migration of pending) {
      db.run(migration.sql)
      insert.run(migration.filename, migrationChecksum(migration.sql), nowMs)
    }
    db.run("COMMIT")
    return { applied, pending: pending.map((m) => m.filename) }
  } catch (error) {
    try {
      db.run("ROLLBACK")
    } catch {
      // The failed statement may already have ended the transaction.
    }
    throw error
  }
}

export const BUSY_TIMEOUT_MS = 5000

function isBusyError(error: unknown): boolean {
  const code = (error as { code?: unknown } | null)?.code
  return typeof code === "string" && code.startsWith("SQLITE_BUSY")
}

function journalMode(db: Database, sql: string): string {
  const row = db.query<{ journal_mode: string }, []>(sql).get()
  return row?.journal_mode?.toLowerCase() ?? ""
}

/**
 * Switches the database to WAL, retrying on SQLITE_BUSY until `timeoutMs`.
 *
 * Changing the journal mode needs an exclusive lock, and SQLite returns
 * SQLITE_BUSY for it without consulting the busy handler. Two processes
 * opening a fresh database together (e.g. the worker and its health check)
 * otherwise race here. Once one has switched, the mode is persistent, so the
 * cheap read-only check short-circuits every later open.
 */
export function enableWal(
  db: Database,
  timeoutMs: number = BUSY_TIMEOUT_MS,
): void {
  const deadline = Date.now() + timeoutMs
  for (let attempt = 0; ; attempt += 1) {
    try {
      const current = journalMode(db, "PRAGMA journal_mode")
      // In-memory databases cannot use WAL; they have no other process.
      if (current === "wal" || current === "memory") return
      const mode = journalMode(db, "PRAGMA journal_mode = WAL")
      if (mode === "wal") return
      throw new Error(`journal_mode stayed ${mode || "unknown"}`)
    } catch (error) {
      if (!isBusyError(error) || Date.now() >= deadline) throw error
      Bun.sleepSync(Math.min(5 * 2 ** attempt, 100) + Math.random() * 10)
    }
  }
}

export function applyPragmas(db: Database): void {
  db.run(`PRAGMA busy_timeout = ${BUSY_TIMEOUT_MS}`)
  enableWal(db)
  db.run("PRAGMA foreign_keys = ON")
  db.run("PRAGMA synchronous = NORMAL")
}

/**
 * Opens (creating if needed) the telemetry database at `path`, sets pragmas,
 * and applies migrations. Throws on failure; callers that must not fail
 * should use `openTelemetrySink`.
 */
export function openTelemetryDb(
  path: string,
  migrations: readonly Migration[] = MIGRATIONS,
): Database {
  if (path !== ":memory:") {
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 })
  }
  const db = new Database(path, { create: true, strict: true })
  try {
    applyPragmas(db)
    applyMigrations(db, migrations)
    if (path !== ":memory:") restrictToOwner(path)
    return db
  } catch (error) {
    db.close()
    throw error
  }
}

/** Makes the DB and its WAL files owner-only; best effort. */
function restrictToOwner(path: string): void {
  for (const file of [path, `${path}-wal`, `${path}-shm`]) {
    try {
      chmodSync(file, 0o600)
    } catch {
      // Missing WAL files or a read-only filesystem are fine.
    }
  }
}

export interface TelemetrySink {
  /** Records one event. Never throws. */
  record(event: TelemetryEvent): void
  /** Number of events that failed to record. */
  dropped(): number
  close(): void
}

export interface SinkDefaults {
  host?: string
  pid?: number
  version?: string
}

export type StderrWriter = (line: string) => void

const defaultStderr: StderrWriter = (line) => {
  try {
    process.stderr.write(`${line}\n`)
  } catch {
    // Nothing left to report to.
  }
}

function jsonOrNull(value: unknown): string | null {
  if (value === undefined || value === null) return null
  return JSON.stringify(value)
}

/** Maps an event plus sink defaults to the positional insert parameters. */
export function eventToParams(
  event: TelemetryEvent,
  defaults: SinkDefaults = {},
): (string | number | null)[] {
  const tsMs = event.tsMs ?? Date.now()
  return [
    event.eventId ?? uuidv7(tsMs),
    event.requestId ?? null,
    event.runId,
    event.jobId ?? null,
    tsMs,
    event.durationMs ?? null,
    event.event,
    event.subsystem,
    event.command ?? null,
    event.accountId ?? null,
    event.profile ?? null,
    event.method ?? null,
    event.attempt ?? null,
    event.outcome,
    event.errorCode ?? null,
    event.err ?? null,
    event.waitS ?? null,
    jsonOrNull(event.req),
    jsonOrNull(event.res),
    event.host ?? defaults.host ?? null,
    event.pid ?? defaults.pid ?? null,
    event.version ?? defaults.version ?? null,
  ]
}

const INSERT_SQL = `INSERT INTO event (
  event_id, request_id, run_id, job_id, ts_ms, duration_ms, event, subsystem,
  command, account_id, profile, method, attempt, outcome, error_code, err,
  wait_s, req, res, host, pid, version
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`

export class SqliteTelemetrySink implements TelemetrySink {
  private droppedCount = 0
  private insert: Statement | null = null
  private readonly defaults: SinkDefaults

  constructor(
    private readonly db: Database,
    defaults: SinkDefaults = {},
    private readonly stderr: StderrWriter = defaultStderr,
  ) {
    this.defaults = {
      host: defaults.host ?? safeHostname(),
      pid: defaults.pid ?? process.pid,
      version: defaults.version,
    }
  }

  record(event: TelemetryEvent): void {
    try {
      this.insert ??= this.db.prepare(INSERT_SQL)
      this.insert.run(...eventToParams(event, this.defaults))
    } catch (error) {
      this.droppedCount += 1
      this.stderr(
        `tg-sai telemetry: dropped ${String(event?.event)} event ` +
          `(${this.droppedCount} dropped): ${errorMessage(error)}`,
      )
    }
  }

  dropped(): number {
    return this.droppedCount
  }

  close(): void {
    try {
      this.insert?.finalize()
      this.insert = null
      this.db.close(true)
    } catch {
      // Already closed.
    }
  }
}

/** Sink for disabled telemetry: accepts and discards every event. */
export class NoopSink implements TelemetrySink {
  record(_event: TelemetryEvent): void {}
  dropped(): number {
    return 0
  }
  close(): void {}
}

export interface OpenSinkOptions extends SinkDefaults {
  path?: string
  stderr?: StderrWriter
}

/**
 * Returns a sink for the environment. Never throws: telemetry disabled via
 * `TG_SAI_TELEMETRY=off`, or a database that fails to open, yields a
 * `NoopSink` (the latter after one stderr line).
 */
export function openTelemetrySink(
  env: ProfileStoreEnv = process.env,
  options: OpenSinkOptions = {},
): TelemetrySink {
  if (telemetryDisabled(env)) return new NoopSink()
  const stderr = options.stderr ?? defaultStderr
  const path = options.path ?? telemetryDbPath(env)
  try {
    const db = openTelemetryDb(path)
    return new SqliteTelemetrySink(db, options, stderr)
  } catch (error) {
    stderr(`tg-sai telemetry: disabled, cannot open ${path}: ` +
      errorMessage(error))
    return new NoopSink()
  }
}

function safeHostname(): string | undefined {
  try {
    return hostname()
  } catch {
    return undefined
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
