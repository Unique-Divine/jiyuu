/**
 * `tg-sai events`: read-only views over the telemetry database. Output is a
 * TOON table by default and the same projection as JSON with `--json`.
 */
import type { Database } from "bun:sqlite"
import type { Command } from "commander"
import { existsSync, statSync } from "node:fs"

import type { ProfileStoreEnv } from "../profiles"
import {
  LOG_LEVELS,
  telemetryDbPath,
  type EventRow,
  type LogLevel,
} from "./db"
import {
  countEvents,
  eventStats,
  exportEvents,
  floodReport,
  logEvents,
  openTelemetryReadOnly,
  parseCountBy,
  recentRuns,
  showTimeline,
  tailEvents,
  uncertainCalls,
  type CountLayer,
} from "./queries"

// ---------------------------------------------------------------------------
// Durations and option parsing

const DURATION_UNITS_MS: Readonly<Record<string, number>> = {
  ms: 1,
  s: 1000,
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
  w: 604_800_000,
}

/**
 * Parses `30m`, `24h`, `7d`, `1.5h`, `90s`, `2w`, or `500ms` into
 * milliseconds. Throws on anything else, including zero or negative values.
 */
export function parseDuration(value: string): number {
  const match = /^(\d+(?:\.\d+)?)(ms|s|m|h|d|w)$/.exec(value.trim())
  if (!match) {
    throw new Error(
      `Invalid duration "${value}"; use a number and unit, e.g. 30m, 24h, 7d`,
    )
  }
  const amount = Number(match[1])
  const unit = DURATION_UNITS_MS[match[2] ?? ""] ?? 0
  const ms = Math.round(amount * unit)
  if (!(ms > 0)) throw new Error(`Duration must be positive: "${value}"`)
  return ms
}

/** `now - duration`, the inclusive lower bound for `--since`. */
export function sinceCutoff(value: string, nowMs: number): number {
  return nowMs - parseDuration(value)
}

export function parsePositiveInt(value: string, flag: string): number {
  const n = Number(value)
  if (!Number.isSafeInteger(n) || n <= 0) {
    throw new Error(`${flag} must be a positive integer, got "${value}"`)
  }
  return n
}

export function parseAccountId(value: string): number {
  const n = Number(value)
  if (!/^\d+$/.test(value.trim()) || !Number.isSafeInteger(n)) {
    throw new Error(`--account must be a numeric Telegram user id: "${value}"`)
  }
  return n
}

export function parseLevel(value: string): LogLevel {
  const level = value.trim().toLowerCase()
  if ((LOG_LEVELS as readonly string[]).includes(level)) {
    return level as LogLevel
  }
  throw new Error(
    `--level must be one of ${LOG_LEVELS.join(", ")}, got "${value}"`,
  )
}

export function parseLayer(value: string): CountLayer {
  if (value === "attempt" || value === "call") return value
  throw new Error(`--layer must be attempt or call, got "${value}"`)
}

// ---------------------------------------------------------------------------
// TOON encoding

export type Cell = string | number | boolean | null | undefined | object

const NUMERIC_LIKE = /^-?(?:\d+(?:\.\d+)?(?:e[+-]?\d+)?|0\d+)$/i
const NEEDS_QUOTE = /[:"\\[\]{},\n\r\t]/

function quoteToon(value: string): string {
  const escaped = value
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r")
    .replace(/\t/g, "\\t")
  return `"${escaped}"`
}

/** Encodes one TOON primitive, quoting strings that would be ambiguous. */
export function toonValue(value: Cell): string {
  if (value === null || value === undefined) return "null"
  if (typeof value === "number") {
    return Number.isFinite(value) ? String(value) : "null"
  }
  if (typeof value === "boolean") return String(value)
  if (typeof value === "object") return quoteToon(JSON.stringify(value))
  const needsQuote =
    value === "" ||
    value !== value.trim() ||
    value === "true" ||
    value === "false" ||
    value === "null" ||
    value.startsWith("-") ||
    NUMERIC_LIKE.test(value) ||
    NEEDS_QUOTE.test(value)
  return needsQuote ? quoteToon(value) : value
}

/**
 * Encodes rows as a TOON tabular array: `name[N]{a,b}:` then one indented,
 * comma-separated line per row. An empty table is `name[0]:`.
 */
export function toonTable(
  name: string,
  fields: readonly string[],
  rows: readonly Record<string, Cell>[],
): string {
  if (rows.length === 0) return `${name}[0]:`
  const header = `${name}[${rows.length}]{${fields.join(",")}}:`
  const lines = rows.map(
    (row) => "  " + fields.map((f) => toonValue(row[f])).join(","),
  )
  return [header, ...lines].join("\n")
}

/** Encodes `key: value` lines for a flat object. */
export function toonObject(entries: Record<string, Cell>): string {
  return Object.entries(entries)
    .map(([key, value]) => `${key}: ${toonValue(value)}`)
    .join("\n")
}

// ---------------------------------------------------------------------------
// Projections shared by TOON and JSON output

export function isoTime(ms: number | null | undefined): string | null {
  if (ms === null || ms === undefined) return null
  return new Date(ms).toISOString()
}

function parseJsonColumn(value: string | null): unknown {
  if (value === null) return null
  try {
    return JSON.parse(value)
  } catch {
    return value
  }
}

export const TAIL_FIELDS = [
  "ts",
  "event",
  "outcome",
  "account_id",
  "method",
  "attempt",
  "duration_ms",
  "error_code",
  "err",
  "wait_s",
  "request_id",
] as const

export function tailProjection(row: EventRow): Record<string, Cell> {
  return {
    ts: isoTime(row.ts_ms),
    event: row.event,
    outcome: row.outcome,
    account_id: row.account_id,
    method: row.method,
    attempt: row.attempt,
    duration_ms: row.duration_ms,
    error_code: row.error_code,
    err: row.err,
    wait_s: row.wait_s,
    request_id: row.request_id,
  }
}

export const SHOW_FIELDS = [
  "ts",
  "event",
  "outcome",
  "method",
  "attempt",
  "duration_ms",
  "error_code",
  "err",
  "wait_s",
  "request_id",
  "job_id",
  "level",
  "msg",
  "req",
  "res",
  "attrs",
] as const

export function showProjection(row: EventRow): Record<string, Cell> {
  return {
    ts: isoTime(row.ts_ms),
    event: row.event,
    outcome: row.outcome,
    level: row.level ?? null,
    msg: row.msg ?? null,
    method: row.method,
    attempt: row.attempt,
    duration_ms: row.duration_ms,
    error_code: row.error_code,
    err: row.err,
    wait_s: row.wait_s,
    request_id: row.request_id,
    job_id: row.job_id,
    req: parseJsonColumn(row.req) as Cell,
    res: parseJsonColumn(row.res) as Cell,
    attrs: parseJsonColumn(row.attrs ?? null) as Cell,
  }
}

export const LOG_FIELDS = [
  "ts",
  "level",
  "event",
  "subsystem",
  "msg",
  "attrs",
  "account_id",
  "job_id",
  "run_id",
] as const

export function logProjection(row: EventRow): Record<string, Cell> {
  return {
    ts: isoTime(row.ts_ms),
    level: row.level,
    event: row.event,
    subsystem: row.subsystem,
    msg: row.msg,
    attrs: parseJsonColumn(row.attrs) as Cell,
    account_id: row.account_id,
    job_id: row.job_id,
    run_id: row.run_id,
  }
}

export const UNCERTAIN_FIELDS = [
  "ts",
  "age_s",
  "account_id",
  "method",
  "command",
  "run_id",
  "request_id",
] as const

export const RUN_FIELDS = [
  "started",
  "run_id",
  "command",
  "subsystem",
  "outcome",
  "error_code",
  "err",
  "duration_ms",
  "calls",
  "errors",
  "attempts",
  "ticks",
] as const

export const FLOOD_FIELDS = [
  "event",
  "account_id",
  "method",
  "wait_s",
  "count",
  "latest",
] as const

export const ACTIVE_FLOOD_FIELDS = [
  "account_id",
  "method",
  "wait_s",
  "started",
  "ends",
  "remaining_s",
  "request_id",
] as const

// ---------------------------------------------------------------------------
// Command wiring

export interface EventsCliContext {
  env?: ProfileStoreEnv
  /** Overrides `telemetryDbPath(env)`. */
  dbPath?: string
  now?: () => number
  stdout?: (text: string) => void
  stderr?: (text: string) => void
}

interface ResolvedContext {
  path: string
  now: () => number
  out: (text: string) => void
  err: (text: string) => void
}

function resolveContext(ctx: EventsCliContext): ResolvedContext {
  return {
    path: ctx.dbPath ?? telemetryDbPath(ctx.env ?? process.env),
    now: ctx.now ?? Date.now,
    out: ctx.stdout ?? ((text) => process.stdout.write(`${text}\n`)),
    err: ctx.stderr ?? ((text) => process.stderr.write(`${text}\n`)),
  }
}

/**
 * Opens the database read-only and runs `fn`. A missing database prints a
 * note to stderr and returns without error: nothing has been recorded yet.
 */
export function withReadOnlyDb(
  ctx: ResolvedContext,
  fn: (db: Database) => void,
): void {
  if (!existsSync(ctx.path)) {
    ctx.err(
      `No telemetry database at ${ctx.path}; nothing has been recorded yet.`,
    )
    return
  }
  const db = openTelemetryReadOnly(ctx.path)
  try {
    fn(db)
  } finally {
    db.close()
  }
}

function emit(
  ctx: ResolvedContext,
  json: boolean | undefined,
  name: string,
  fields: readonly string[],
  rows: Record<string, Cell>[],
): void {
  if (json) ctx.out(JSON.stringify(rows, null, 2))
  else ctx.out(toonTable(name, fields, rows))
}

interface TailCliOptions {
  limit: string
  account?: string
  method?: string
  errors?: boolean
  run?: string
  since?: string
  json?: boolean
}

interface LogsCliOptions {
  since: string
  level?: string
  subsystem?: string
  run?: string
  job?: string
  limit: string
  json?: boolean
}

interface SinceCliOptions {
  since: string
  account?: string
  json?: boolean
}

interface CountsCliOptions extends SinceCliOptions {
  by: string
  layer: string
}

/**
 * Registers `tg-sai events` and its subcommands on `program`. `context` is
 * for tests; production uses the environment, clock, and stdio.
 */
export function registerEventsCommand(
  program: Command,
  context: EventsCliContext = {},
): Command {
  const ctx = resolveContext(context)
  const events = program
    .command("events")
    .description(
      "Read the local telemetry database: Telegram calls and app logs " +
        "(read-only)",
    )
    .addHelpText(
      "after",
      `
Examples:
  tg-sai events tail --account 8274304175 --errors
  tg-sai events logs --since 24h --level warn --subsystem worker
  tg-sai events show <request_id|run_id|job_id>
  tg-sai events counts --since 24h --by method,outcome
  tg-sai events floods --since 7d
  tg-sai events export --since 7d > events.jsonl

Output is TOON by default; add --json for JSON. Durations: 30m, 24h, 7d.
`,
    )
    .action(() => {
      events.help({ error: false })
    })

  events
    .command("tail")
    .description("Newest calls, attempts, and flood waits, one per line")
    .option("--limit <n>", "Maximum rows", "50")
    .option("--account <id>", "Only this Telegram account id")
    .option("--method <name>", "Only this TL method, e.g. messages.getDialogs")
    .option("--errors", "Only failed rows and flood waits")
    .option("--run <run_id>", "Only rows from this run")
    .option("--since <duration>", "Only rows newer than this, e.g. 24h")
    .option("--json", "Print JSON")
    .action((options: TailCliOptions) => {
      const limit = parsePositiveInt(options.limit, "--limit")
      const filter = {
        limit,
        accountId:
          options.account === undefined
            ? undefined
            : parseAccountId(options.account),
        method: options.method,
        runId: options.run,
        errorsOnly: options.errors,
        sinceMs:
          options.since === undefined
            ? undefined
            : sinceCutoff(options.since, ctx.now()),
      }
      withReadOnlyDb(ctx, (db) => {
        const rows = tailEvents(db, filter).map(tailProjection)
        emit(ctx, options.json, "events", TAIL_FIELDS, rows)
      })
    })

  events
    .command("logs")
    .description("Newest app log rows (rows with a level), one per line")
    .option("--since <duration>", "Window, e.g. 24h", "24h")
    .option(
      "--level <level>",
      "Minimum level: debug, info, warn, or error",
    )
    .option("--subsystem <name>", "Only this subsystem: worker, health, cli")
    .option("--run <run_id>", "Only rows from this run")
    .option("--job <job_id>", "Only rows for this job (setup request id)")
    .option("--limit <n>", "Maximum rows", "100")
    .option("--json", "Print JSON")
    .action((options: LogsCliOptions) => {
      const query = {
        limit: parsePositiveInt(options.limit, "--limit"),
        sinceMs: sinceCutoff(options.since, ctx.now()),
        minLevel:
          options.level === undefined ? undefined : parseLevel(options.level),
        subsystem: options.subsystem,
        runId: options.run,
        jobId: options.job,
      }
      withReadOnlyDb(ctx, (db) => {
        const rows = logEvents(db, query).map(logProjection)
        emit(ctx, options.json, "logs", LOG_FIELDS, rows)
      })
    })

  events
    .command("show <id>")
    .description("Timeline of every row sharing a request, run, or job id")
    .option("--json", "Print JSON")
    .action((id: string, options: { json?: boolean }) => {
      withReadOnlyDb(ctx, (db) => {
        const rows = showTimeline(db, id).map(showProjection)
        if (rows.length === 0) ctx.err(`No telemetry rows match ${id}`)
        emit(ctx, options.json, "timeline", SHOW_FIELDS, rows)
      })
    })

  events
    .command("counts")
    .description("Row counts grouped by fields")
    .option("--since <duration>", "Window, e.g. 24h", "24h")
    .option("--by <fields>", "Comma-separated group fields", "method,outcome")
    .option(
      "--layer <layer>",
      "attempt (requests sent to Telegram) or call (finished calls)",
      "attempt",
    )
    .option("--account <id>", "Only this Telegram account id")
    .option("--json", "Print JSON")
    .action((options: CountsCliOptions) => {
      const by = parseCountBy(options.by)
      const layer = parseLayer(options.layer)
      const sinceMs = sinceCutoff(options.since, ctx.now())
      const accountId =
        options.account === undefined
          ? undefined
          : parseAccountId(options.account)
      withReadOnlyDb(ctx, (db) => {
        const rows = countEvents(db, { sinceMs, accountId, by, layer })
        emit(ctx, options.json, "counts", [...by, "count"], rows)
      })
    })

  events
    .command("floods")
    .description("Flood waits by method and wait, plus waits still active")
    .option("--since <duration>", "Window, e.g. 7d", "7d")
    .option("--account <id>", "Only this Telegram account id")
    .option("--json", "Print JSON")
    .action((options: SinceCliOptions) => {
      const nowMs = ctx.now()
      const sinceMs = sinceCutoff(options.since, nowMs)
      const accountId =
        options.account === undefined
          ? undefined
          : parseAccountId(options.account)
      withReadOnlyDb(ctx, (db) => {
        const report = floodReport(db, { sinceMs, accountId, nowMs })
        const floods = report.floods.map((row) => ({
          event: row.event,
          account_id: row.account_id,
          method: row.method,
          wait_s: row.wait_s,
          count: row.count,
          latest: isoTime(row.latest_ms),
        }))
        const active = report.active.map((row) => ({
          account_id: row.account_id,
          method: row.method,
          wait_s: row.wait_s,
          started: isoTime(row.started_ms),
          ends: isoTime(row.ends_ms),
          remaining_s: Math.ceil((row.ends_ms - nowMs) / 1000),
          request_id: row.request_id,
        }))
        if (options.json) {
          ctx.out(JSON.stringify({ floods, active }, null, 2))
          return
        }
        ctx.out(toonTable("floods", FLOOD_FIELDS, floods))
        ctx.out(toonTable("active", ACTIVE_FLOOD_FIELDS, active))
      })
    })

  events
    .command("uncertain")
    .description("Calls that started and never recorded a completion")
    .option("--since <duration>", "Window, e.g. 7d", "7d")
    .option("--json", "Print JSON")
    .action((options: { since: string; json?: boolean }) => {
      const nowMs = ctx.now()
      const sinceMs = sinceCutoff(options.since, nowMs)
      withReadOnlyDb(ctx, (db) => {
        const rows = uncertainCalls(db, { sinceMs }).map((row) => ({
          ts: isoTime(row.ts_ms),
          age_s: Math.floor((nowMs - row.ts_ms) / 1000),
          account_id: row.account_id,
          method: row.method,
          command: row.command,
          run_id: row.run_id,
          request_id: row.request_id,
        }))
        emit(ctx, options.json, "uncertain", UNCERTAIN_FIELDS, rows)
      })
    })

  events
    .command("runs")
    .description("Recent runs with outcome, duration, and error count")
    .option("--limit <n>", "Maximum runs", "20")
    .option("--json", "Print JSON")
    .action((options: { limit: string; json?: boolean }) => {
      const limit = parsePositiveInt(options.limit, "--limit")
      withReadOnlyDb(ctx, (db) => {
        const rows = recentRuns(db, limit).map((row) => ({
          started: isoTime(row.started_ms),
          run_id: row.run_id,
          command: row.command,
          subsystem: row.subsystem,
          outcome: row.outcome ?? "unfinished",
          error_code: row.error_code,
          err: row.err,
          duration_ms: row.duration_ms,
          calls: row.calls,
          errors: row.errors,
          attempts: row.attempts,
          ticks: row.ticks,
        }))
        emit(ctx, options.json, "runs", RUN_FIELDS, rows)
      })
    })

  events
    .command("stats")
    .description("Database path, size, row count, and rows per event kind")
    .option("--json", "Print JSON")
    .action((options: { json?: boolean }) => {
      withReadOnlyDb(ctx, (db) => {
        const stats = eventStats(db)
        const summary = {
          path: ctx.path,
          size_bytes: fileSize(ctx.path),
          wal_bytes: fileSize(`${ctx.path}-wal`),
          rows: stats.rows,
          log_rows: stats.log_rows,
          oldest: isoTime(stats.oldest_ms),
          newest: isoTime(stats.newest_ms),
        }
        if (options.json) {
          ctx.out(
            JSON.stringify(
              { ...summary, kinds: stats.kinds, levels: stats.levels },
              null,
              2,
            ),
          )
          return
        }
        ctx.out(toonObject(summary))
        const kinds = stats.kinds.map((k) => ({ event: k.event, rows: k.rows }))
        ctx.out(toonTable("kinds", ["event", "rows"], kinds))
        const levels = stats.levels.map((l) => ({ level: l.level, rows: l.rows }))
        ctx.out(toonTable("levels", ["level", "rows"], levels))
      })
    })

  events
    .command("export")
    .description("Every row as JSONL on stdout, oldest first")
    .option("--since <duration>", "Window, e.g. 7d", "7d")
    .action((options: { since: string }) => {
      const sinceMs = sinceCutoff(options.since, ctx.now())
      withReadOnlyDb(ctx, (db) => {
        for (const row of exportEvents(db, { sinceMs })) {
          ctx.out(JSON.stringify(exportProjection(row)))
        }
      })
    })

  return events
}

/** Full row with `req`/`res` parsed back to JSON values. */
export function exportProjection(row: EventRow): Record<string, unknown> {
  return {
    ...row,
    req: parseJsonColumn(row.req),
    res: parseJsonColumn(row.res),
    ...(row.attrs === undefined
      ? {}
      : { attrs: parseJsonColumn(row.attrs) }),
  }
}

/** File size in bytes, or 0 when the file does not exist. */
export function fileSize(path: string): number {
  try {
    return statSync(path).size
  } catch {
    return 0
  }
}
