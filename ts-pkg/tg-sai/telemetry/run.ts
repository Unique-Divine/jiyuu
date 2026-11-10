import { basename } from "node:path"

import type { ProfileStoreEnv } from "../profiles"
import {
  LOG_EVENT_PREFIX,
  LOG_LEVELS,
  openTelemetrySink,
  RUN_END_EVENT,
  RUN_START_EVENT,
  uuidv7,
} from "./db"
import type {
  LogLevel,
  TelemetryEvent,
  TelemetryOutcome,
  TelemetrySink,
  TelemetrySubsystem,
} from "./db"
import { safeErrorText } from "./middleware"
import type { ClientTelemetry, TelemetryContext } from "./middleware"
import { summarizeTl } from "./summarize"

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
  "migrate-password",
  "ops",
  "prepare",
  "profile",
  "promote-admin",
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
  "--password-prompt",
  "--password-stdin",
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
  "--pass-entry",
  "--password",
  "--password-pass-entry",
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

/** Longest `msg` stored on a log row; longer text is cut. */
export const LOG_MSG_MAX_CHARS = 120

/** Event names reserved for Telegram call and run rows. */
const RESERVED_EVENT = /^(?:tg|run)\./
/** Dotted lowercase names such as `log.worker` or `worker.job.claimed`. */
const LOG_EVENT_NAME = /^[a-z][a-z0-9_]*(?:\.[a-z0-9_-]+)+$/

/** Optional row fields a log line may carry besides its run context. */
export interface LogFields {
  jobId?: string | null
  requestId?: string | null
  accountId?: number | null
  profile?: string | null
  method?: string | null
  durationMs?: number | null
  tsMs?: number
}

/** Writes one app log row; see `logEventFor` for the rules. */
export type LogFn = (
  level: LogLevel,
  event: string,
  msg: string,
  attrs?: unknown,
  fields?: LogFields,
) => void

function isLogLevel(value: unknown): value is LogLevel {
  return (LOG_LEVELS as readonly unknown[]).includes(value)
}

/**
 * `event` when it is a dotted lowercase name outside the reserved `tg.*` and
 * `run.*` namespaces; otherwise `log.<subsystem>`.
 */
export function logEventName(event: string, subsystem: string): string {
  if (LOG_EVENT_NAME.test(event) && !RESERVED_EVENT.test(event)) return event
  return `${LOG_EVENT_PREFIX}${subsystem}`
}

/** Strips control characters and caps the length at `LOG_MSG_MAX_CHARS`. */
export function cleanLogMsg(msg: string): string {
  // biome-ignore lint/suspicious/noControlCharactersInRegex: stripping them
  const text = String(msg).replace(/[\u0000-\u001f\u007f]+/g, " ").trim()
  return text.length > LOG_MSG_MAX_CHARS
    ? text.slice(0, LOG_MSG_MAX_CHARS)
    : text
}

/**
 * Builds one app log row for the `event` table.
 *
 * - `msg` must be a short constant written in tg-sai code, such as
 *   `"setup held for flood wait"`. Never interpolate user data, chat titles,
 *   usernames, message text, or error messages into it; put values in
 *   `attrs` instead.
 * - `attrs` pass through `summarizeTl`: numbers, booleans, and `_`
 *   constructor names survive, every string becomes its length, and
 *   secret-looking keys are dropped.
 * - `outcome` is `error` at error level and `ok` otherwise.
 */
export function logEventFor(
  context: TelemetryContext,
  level: LogLevel,
  event: string,
  msg: string,
  attrs?: unknown,
  fields: LogFields = {},
): TelemetryEvent {
  const safeLevel: LogLevel = isLogLevel(level) ? level : "info"
  return {
    runId: context.runId,
    subsystem: context.subsystem,
    command: context.command,
    version: context.version,
    accountId: fields.accountId ?? context.accountId ?? null,
    profile: fields.profile ?? context.profile ?? null,
    jobId: fields.jobId ?? null,
    requestId: fields.requestId ?? null,
    method: fields.method ?? null,
    durationMs: fields.durationMs ?? null,
    ...(fields.tsMs === undefined ? {} : { tsMs: fields.tsMs }),
    event: logEventName(event, context.subsystem),
    outcome: safeLevel === "error" ? "error" : "ok",
    level: safeLevel,
    msg: cleanLogMsg(msg),
    attrs: attrs === undefined ? null : summarizeTl(attrs),
  }
}

/** Records one log row in `sink`; never throws. */
export function recordLog(
  sink: TelemetrySink,
  context: TelemetryContext,
  ...args: Parameters<LogFn>
): void {
  try {
    sink.record(logEventFor(context, ...args))
  } catch {
    // Logging must not break the caller.
  }
}

export interface StandaloneLogOptions {
  env?: ProfileStoreEnv
  subsystem?: TelemetrySubsystem
  command?: string | null
  version?: string
  /** Test seam; defaults to `openTelemetrySink(env)`, closed after use. */
  sink?: TelemetrySink
}

/**
 * Writes one app log row from anywhere. Uses the active run when this
 * process has one; otherwise opens the telemetry DB for this one row under a
 * fresh `run_id`. Honors `TG_SAI_TELEMETRY=off`. Never throws.
 */
export function log(
  level: LogLevel,
  event: string,
  msg: string,
  attrs?: unknown,
  fields?: LogFields,
  options: StandaloneLogOptions = {},
): void {
  const run = options.sink ? null : activeRun
  if (run) {
    run.log(level, event, msg, attrs, fields)
    return
  }
  try {
    const version = options.version ?? "unknown"
    const sink =
      options.sink ??
      openTelemetrySink(options.env ?? process.env, { version })
    const context: TelemetryContext = {
      runId: uuidv7(),
      subsystem: options.subsystem ?? "cli",
      command: options.command ?? null,
      version,
    }
    recordLog(sink, context, level, event, msg, attrs, fields)
    if (!options.sink) sink.close()
  } catch {
    // Logging must not break the caller.
  }
}

export interface RunEndInput {
  exitCode: number
  error?: unknown
  outcome?: TelemetryOutcome
}

/** One process-level run: identity, sink, and flood threshold. */
export interface TelemetryRun {
  readonly context: TelemetryContext
  readonly sink: TelemetrySink
  readonly floodMaxWaitMs: number
  /** Telemetry for one client; `overrides` add account and profile. */
  client(overrides?: {
    accountId?: number | null
    profile?: string | null
  }): ClientTelemetry
  /** Writes one app log row for this run; see `logEventFor`. */
  log: LogFn
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
  /** Test seam; defaults to `openTelemetrySink(env, {version})`. */
  sink?: TelemetrySink
  stderr?: (line: string) => void
}

let activeRun: TelemetryRun | null = null

/** The run started by this process's entry point, if any. */
export function activeTelemetryRun(): TelemetryRun | null {
  return activeRun
}

/**
 * Opens the sink once and writes `run.start`. Never throws: a DB failure
 * only drops telemetry.
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
  const sink =
    options.sink ?? openTelemetrySink(env, { version: options.version, stderr })
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
    },
  })

  const run: TelemetryRun = {
    context,
    sink,
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
    log(level, event, msg, attrs, fields) {
      if (ended) return
      recordLog(sink, context, level, event, msg, attrs, fields)
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
        res: { exitCode: input.exitCode, dropped: sink.dropped() },
      })
      sink.close()
      if (activeRun === run) activeRun = null
    },
  }
  activeRun = run
  return run
}
