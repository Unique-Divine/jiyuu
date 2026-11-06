import { appendFileSync, mkdirSync, readdirSync } from "node:fs"
import { basename, join } from "node:path"

import { configDir, type ProfileStoreEnv } from "../profiles"
import {
  openTelemetrySink,
  RUN_END_EVENT,
  RUN_START_EVENT,
  uuidv7,
} from "./db"
import type {
  TelemetryEvent,
  TelemetryOutcome,
  TelemetrySink,
  TelemetrySubsystem,
} from "./db"
import { safeErrorText } from "./middleware"
import type { ClientTelemetry, TelemetryContext } from "./middleware"

export const FLOOD_MAX_WAIT_ENV = "TG_SAI_FLOOD_MAX_WAIT_MS"
/** mtcute's own default; CLI commands sleep through waits up to this. */
export const CLI_FLOOD_MAX_WAIT_MS = 10_000
/** The worker is unattended, so it rides out somewhat longer waits. */
export const WORKER_FLOOD_MAX_WAIT_MS = 30_000
/**
 * `ops sai-bootstrap` walks a reviewed batch; sleeping up to five minutes
 * beats stopping the batch. Longer waits stop it (see `floodStopFor`).
 */
export const BOOTSTRAP_FLOOD_MAX_WAIT_MS = 300_000

/**
 * Flood-wait sleep threshold for this process: `TG_SAI_FLOOD_MAX_WAIT_MS`
 * when it is a non-negative integer, else `fallback`, else the subsystem
 * default.
 */
export function floodMaxWaitMs(
  subsystem: TelemetrySubsystem,
  env: ProfileStoreEnv = process.env,
  fallback?: number,
): number {
  const raw = env[FLOOD_MAX_WAIT_ENV]?.trim()
  if (raw && /^\d+$/.test(raw)) return Number(raw)
  if (fallback !== undefined) return fallback
  return subsystem === "worker"
    ? WORKER_FLOOD_MAX_WAIT_MS
    : CLI_FLOOD_MAX_WAIT_MS
}

export const REDACTED = "[redacted]"

/** Subcommand words kept as-is when they lead argv. */
const COMMAND_WORDS = new Set([
  "add",
  "add-bot",
  "add-owner",
  "audit",
  "chat",
  "counts",
  "events",
  "export",
  "find",
  "floods",
  "health",
  "list",
  "ops",
  "prepare",
  "profile",
  "provision",
  "remove",
  "resolve",
  "runs",
  "sai-bootstrap",
  "sai-find",
  "show",
  "stats",
  "status",
  "tail",
  "uncertain",
  "use",
  "worker",
])

/** Flags that take no value; kept as-is. */
const BOOLEAN_FLAGS = new Set([
  "--dry-run",
  "--errors",
  "--help",
  "--interactive",
  "--json",
  "--local",
  "--login",
  "--no-env-write",
  "--no-transfer-owner",
  "--offline",
  "--once",
  "--online",
  "--phone",
  "--qr",
  "--run",
  "--session-output",
  "--version",
  "-i",
])

const NUMBER = /^\d+$/
const CHAT_ID = /^-?\d+$/
/** Durations (`15m`) and ISO-like timestamps; no free text. */
const SINCE = /^\d[0-9A-Za-z:.+-]{0,39}$/

/** Value flags whose values are kept when they have the expected shape. */
const SAFE_VALUE_FLAGS: Record<string, (value: string) => string | null> = {
  "--limit": (v) => (NUMBER.test(v) ? v : null),
  "--interval-ms": (v) => (NUMBER.test(v) ? v : null),
  "--delay-ms": (v) => (NUMBER.test(v) ? v : null),
  "--timeout-ms": (v) => (NUMBER.test(v) ? v : null),
  "--since": (v) => (SINCE.test(v) ? v : null),
  "--chat": (v) => (CHAT_ID.test(v) ? v : null),
  "--chat-id": (v) => (CHAT_ID.test(v) ? v : null),
  "--file": (v) => basename(v) || null,
}

/**
 * Value-taking flags whose values are always redacted. The next token is
 * their value even when it starts with `-` (e.g. `--password -hunter2`).
 */
const REDACTED_VALUE_FLAGS = new Set([
  "--account",
  "--api-hash",
  "--api-id",
  "--archived",
  "--by",
  "--custody-profile",
  "--display-name",
  "--handle",
  "--layer",
  "--method",
  "--name",
  "--out",
  "--password",
  "--profile",
  "--reason",
  "--request-id",
  "--session-string",
  "--src",
  "--target-peer",
  "--target-user",
  "--user-id",
])

function isFlag(arg: string): boolean {
  return arg.startsWith("--") || /^-[A-Za-z]$/.test(arg)
}

/** Kept value for `flag`, or `REDACTED`. */
function flagValue(flag: string, value: string): string {
  const keep = SAFE_VALUE_FLAGS[flag]
  return (keep ? keep(value) : null) ?? REDACTED
}

/**
 * Allowlist view of argv for telemetry rows. Keeps leading subcommand words,
 * flag names, boolean flags, and the values of a few known flags whose
 * values have a safe shape (numbers, durations, numeric chat IDs, the
 * `--file` basename). Every other value and positional becomes `[redacted]`.
 */
export function sanitizeArgv(argv: readonly string[]): string[] {
  const out: string[] = []
  let leading = true
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i] ?? ""
    if (arg === "--") {
      out.push(arg, ...argv.slice(i + 1).map(() => REDACTED))
      break
    }
    if (!isFlag(arg)) {
      const keep = leading && COMMAND_WORDS.has(arg)
      if (!keep) leading = false
      out.push(keep ? arg : REDACTED)
      continue
    }
    leading = false
    const eq = arg.indexOf("=")
    const flag = eq === -1 ? arg : arg.slice(0, eq)
    if (eq !== -1) {
      out.push(`${flag}=${flagValue(flag, arg.slice(eq + 1))}`)
      continue
    }
    out.push(flag)
    if (BOOLEAN_FLAGS.has(flag)) continue
    const takesValue = flag in SAFE_VALUE_FLAGS || REDACTED_VALUE_FLAGS.has(flag)
    const next = argv[i + 1]
    // Unknown flags: a following positional is redacted on its own pass.
    if (!takesValue || next === undefined) continue
    out.push(flagValue(flag, next))
    i += 1
  }
  return out
}

/** Picks `<prefix>-NN.jsonl`, one past the highest existing index. */
export function nextJsonlName(files: readonly string[], prefix: string): string {
  const escaped = prefix.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
  const matcher = new RegExp(`^${escaped}-(\\d+)\\.jsonl$`)
  const maxIndex = files.reduce((max, file) => {
    const match = file.match(matcher)
    return match ? Math.max(max, Number(match[1])) : max
  }, -1)
  return `${prefix}-${String(maxIndex + 1).padStart(2, "0")}.jsonl`
}

/** Creates the logs dir if needed and returns the next run log path. */
export function nextJsonlLogPathSync(dir: string, prefix: string): string {
  mkdirSync(dir, { recursive: true, mode: 0o700 })
  return join(dir, nextJsonlName(readdirSync(dir), prefix))
}

/** Appends one JSON line; returns false instead of throwing. */
export function appendJsonlLine(
  path: string,
  fields: Record<string, unknown>,
): boolean {
  try {
    appendFileSync(path, `${JSON.stringify(fields)}\n`, {
      encoding: "utf8",
      mode: 0o600,
    })
    return true
  } catch {
    return false
  }
}

const SNAKE_FIELDS: [keyof TelemetryEvent, string][] = [
  ["eventId", "event_id"],
  ["requestId", "request_id"],
  ["runId", "run_id"],
  ["jobId", "job_id"],
  ["tsMs", "ts_ms"],
  ["durationMs", "duration_ms"],
  ["event", "event"],
  ["subsystem", "subsystem"],
  ["command", "command"],
  ["accountId", "account_id"],
  ["profile", "profile"],
  ["method", "method"],
  ["attempt", "attempt"],
  ["outcome", "outcome"],
  ["errorCode", "error_code"],
  ["err", "err"],
  ["waitS", "wait_s"],
  ["req", "req"],
  ["res", "res"],
  ["version", "version"],
]

/** JSONL shape of a telemetry event: the DB column names, nulls dropped. */
export function telemetryJsonlFields(
  event: TelemetryEvent,
): Record<string, unknown> {
  const tsMs = event.tsMs ?? Date.now()
  const fields: Record<string, unknown> = {
    ts: new Date(tsMs).toISOString(),
    type: "telemetry",
  }
  for (const [key, column] of SNAKE_FIELDS) {
    const value = event[key]
    if (value !== undefined && value !== null) fields[column] = value
  }
  return fields
}

/**
 * Sink that assigns `eventId`/`tsMs` once, then records the event in `sink`
 * and, when `jsonlPath` is set, appends it to the run log with the same ids.
 */
export function teeSink(
  sink: TelemetrySink,
  jsonlPath: () => string | null,
): TelemetrySink {
  return {
    record(event) {
      let full = event
      try {
        const tsMs = event.tsMs ?? Date.now()
        full = { ...event, tsMs, eventId: event.eventId ?? uuidv7(tsMs) }
      } catch {
        // Fall through with the original event.
      }
      try {
        sink.record(full)
      } catch {
        // Sinks should not throw; tolerate one that does.
      }
      try {
        const path = jsonlPath()
        if (path) appendJsonlLine(path, telemetryJsonlFields(full))
      } catch {
        // Ignore.
      }
    },
    dropped: () => sink.dropped(),
    close: () => sink.close(),
  }
}

export interface RunEndInput {
  exitCode: number
  error?: unknown
  outcome?: TelemetryOutcome
}

/** One process-level run: identity, sink, run log, and flood threshold. */
export interface TelemetryRun {
  readonly context: TelemetryContext
  readonly sink: TelemetrySink
  readonly jsonlPath: string | null
  readonly floodMaxWaitMs: number
  /** Telemetry for one client; `overrides` add account and profile. */
  client(overrides?: {
    accountId?: number | null
    profile?: string | null
  }): ClientTelemetry
  /** Appends a non-telemetry line to the run log, stamped with run_id. */
  appendJsonl(fields: Record<string, unknown>): void
  /** Records the run as aborted (signal) when it ends. */
  markAborted(): void
  /** Writes `run.end` and closes the sink. Idempotent. */
  end(input: RunEndInput): void
}

export interface StartRunOptions {
  subsystem: TelemetrySubsystem
  command: string | null
  argv: readonly string[]
  version: string
  env?: ProfileStoreEnv
  /** Flood-wait threshold when `TG_SAI_FLOOD_MAX_WAIT_MS` is unset. */
  floodMaxWaitMs?: number
  /** Run log file prefix; `null` disables the JSONL run log. */
  logPrefix?: string | null
  logsDir?: string
  /** Test seam; defaults to `openTelemetrySink(env, {version})`. */
  sink?: TelemetrySink
  stderr?: (line: string) => void
}

let activeRun: TelemetryRun | null = null

/** Full error text for the operator's terminal (not for telemetry rows). */
function stderrText(error: unknown): string {
  try {
    return error instanceof Error ? error.message : String(error)
  } catch {
    return "unprintable error"
  }
}

/** The run started by this process's entry point, if any. */
export function activeTelemetryRun(): TelemetryRun | null {
  return activeRun
}

/**
 * Opens the sink once, creates the JSONL run log, and writes `run.start`.
 * Never throws: a log or DB failure only drops that output.
 */
export function startTelemetryRun(options: StartRunOptions): TelemetryRun {
  const env = options.env ?? process.env
  const stderr =
    options.stderr ??
    ((line: string) => {
      try {
        process.stderr.write(`${line}\n`)
      } catch {
        // Nothing to report to.
      }
    })
  const runId = uuidv7()
  const context: TelemetryContext = {
    runId,
    subsystem: options.subsystem,
    command: options.command,
    version: options.version,
  }
  let jsonlPath: string | null = null
  if (options.logPrefix !== null) {
    try {
      jsonlPath = nextJsonlLogPathSync(
        options.logsDir ?? join(configDir(env), "logs"),
        options.logPrefix ?? (options.command ?? "run").replace(/\s+/g, "-"),
      )
    } catch (error) {
      stderr(`tg-sai telemetry: run log disabled: ${stderrText(error)}`)
    }
  }
  const dbSink =
    options.sink ?? openTelemetrySink(env, { version: options.version, stderr })
  const sink = teeSink(dbSink, () => jsonlPath)
  const maxWait = floodMaxWaitMs(
    options.subsystem,
    env,
    options.floodMaxWaitMs,
  )
  const startedAt = Date.now()
  let ended = false
  let aborted = false

  const base = () => ({
    runId,
    subsystem: context.subsystem,
    command: context.command,
    version: context.version,
  })

  sink.record({
    ...base(),
    event: RUN_START_EVENT,
    outcome: "started",
    req: {
      argv: sanitizeArgv(options.argv),
      floodMaxWaitMs: maxWait,
      ...(jsonlPath ? { jsonlPath } : {}),
    },
  })

  const run: TelemetryRun = {
    context,
    sink,
    jsonlPath,
    floodMaxWaitMs: maxWait,
    client(overrides = {}) {
      const clientContext: TelemetryContext = {
        ...context,
        accountId: overrides.accountId ?? null,
        profile: overrides.profile ?? null,
      }
      return {
        sink,
        context: () => clientContext,
        floodWaiter: { maxWait },
      }
    },
    appendJsonl(fields) {
      if (jsonlPath) appendJsonlLine(jsonlPath, { ...fields, run_id: runId })
    },
    markAborted() {
      aborted = true
    },
    end(input) {
      if (ended) return
      ended = true
      const outcome: TelemetryOutcome =
        input.outcome ??
        (aborted ? "aborted" : input.exitCode === 0 ? "ok" : "error")
      sink.record({
        ...base(),
        event: RUN_END_EVENT,
        outcome,
        durationMs: Date.now() - startedAt,
        // Process exit code; read by `tg-sai events runs`.
        errorCode: input.exitCode,
        err: input.error === undefined ? null : safeErrorText(input.error),
        res: { exitCode: input.exitCode, dropped: dbSink.dropped() },
      })
      sink.close()
      if (activeRun === run) activeRun = null
    },
  }
  activeRun = run
  return run
}
