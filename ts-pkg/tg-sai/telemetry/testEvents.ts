/**
 * Realistic telemetry rows for query and CLI tests. Written through the real
 * sink into a real database file; no mocks.
 */
import {
  SqliteTelemetrySink,
  openTelemetryDb,
  type TelemetryEvent,
} from "./db"

/** Fixed clock for tests: 2026-11-06T12:00:00Z. */
export const NOW_MS = Date.UTC(2026, 10, 6, 12, 0, 0)

export const PERSONAL_ACCOUNT = 111222333
export const CUSTODY_ACCOUNT = 8274304175

export const IDS = {
  runCli: "run-cli-0001",
  runWorker: "run-worker-0002",
  jobWorker: "job-deal-0003",
  reqFlooded: "req-flooded-ok",
  reqError: "req-admin-required",
  reqUncertain: "req-uncertain",
  reqSynthetic: "req-synthetic-flood",
  reqOld: "req-old",
} as const

const MIN = 60_000
const HOUR = 60 * MIN

function cliEvent(overrides: Partial<TelemetryEvent>): TelemetryEvent {
  return {
    runId: IDS.runCli,
    event: "tg.rpc.attempt",
    subsystem: "cli",
    command: "ops sai-find",
    accountId: PERSONAL_ACCOUNT,
    profile: "personal",
    outcome: "ok",
    host: "workstation",
    pid: 4242,
    version: "0.0.6",
    ...overrides,
  }
}

function workerEvent(overrides: Partial<TelemetryEvent>): TelemetryEvent {
  return {
    runId: IDS.runWorker,
    jobId: IDS.jobWorker,
    event: "tg.rpc.attempt",
    subsystem: "worker",
    command: "custody-worker",
    accountId: CUSTODY_ACCOUNT,
    profile: "SaiTeam_AI",
    outcome: "ok",
    host: "sai-deal-desk",
    pid: 777,
    version: "0.0.6",
    ...overrides,
  }
}

/** Every seeded event, in insertion order. Event ids sort with time. */
export function seedEvents(nowMs: number = NOW_MS): TelemetryEvent[] {
  const cliStart = nowMs - 2 * HOUR
  const workerStart = nowMs - 30 * MIN
  let seq = 0
  const id = (): string => `evt-${String(seq++).padStart(4, "0")}`
  const events: TelemetryEvent[] = [
    // CLI run: one call that hit a short flood wait and then succeeded, and
    // one call that failed.
    cliEvent({ event: "run.start", jobId: null, tsMs: cliStart }),
    cliEvent({
      event: "tg.rpc.call",
      requestId: IDS.reqFlooded,
      method: "messages.getDialogs",
      outcome: "started",
      tsMs: cliStart + 1000,
      req: { _: "messages.getDialogs", limit: 100 },
    }),
    cliEvent({
      requestId: IDS.reqFlooded,
      method: "messages.getDialogs",
      attempt: 1,
      outcome: "error",
      errorCode: 420,
      err: "FLOOD_WAIT_3",
      waitS: 3,
      durationMs: 80,
      tsMs: cliStart + 1100,
    }),
    cliEvent({
      event: "tg.flood.wait",
      requestId: IDS.reqFlooded,
      method: "messages.getDialogs",
      outcome: "error",
      waitS: 3,
      tsMs: cliStart + 1200,
    }),
    cliEvent({
      requestId: IDS.reqFlooded,
      method: "messages.getDialogs",
      attempt: 2,
      outcome: "ok",
      durationMs: 95,
      tsMs: cliStart + 4300,
      res: { _: "messages.dialogsSlice", count: 412 },
    }),
    cliEvent({
      event: "tg.rpc.call",
      requestId: IDS.reqFlooded,
      method: "messages.getDialogs",
      outcome: "ok",
      durationMs: 3400,
      tsMs: cliStart + 4400,
    }),
    cliEvent({
      event: "tg.rpc.call",
      requestId: IDS.reqError,
      method: "channels.editAdmin",
      outcome: "started",
      tsMs: cliStart + 5000,
    }),
    cliEvent({
      requestId: IDS.reqError,
      method: "channels.editAdmin",
      attempt: 1,
      outcome: "error",
      errorCode: 400,
      err: "CHAT_ADMIN_REQUIRED",
      durationMs: 60,
      tsMs: cliStart + 5100,
    }),
    cliEvent({
      event: "tg.rpc.call",
      requestId: IDS.reqError,
      method: "channels.editAdmin",
      outcome: "error",
      errorCode: 400,
      err: "CHAT_ADMIN_REQUIRED",
      durationMs: 120,
      tsMs: cliStart + 5120,
    }),
    cliEvent({
      event: "run.end",
      outcome: "ok",
      durationMs: 10_000,
      tsMs: cliStart + 10_000,
    }),

    // Worker run with no run.end: one call lost mid-flight and one that
    // waited on a remembered flood without sending a request.
    workerEvent({ event: "run.start", jobId: null, tsMs: workerStart }),
    workerEvent({ event: "worker.tick", tsMs: workerStart + 1000 }),
    workerEvent({
      event: "tg.rpc.call",
      requestId: IDS.reqUncertain,
      method: "channels.createChannel",
      outcome: "started",
      tsMs: workerStart + 60_000,
      req: { _: "channels.createChannel", title: { len: 18 } },
    }),
    workerEvent({
      requestId: IDS.reqUncertain,
      method: "channels.createChannel",
      attempt: 1,
      outcome: "timeout",
      durationMs: 20_000,
      tsMs: workerStart + 80_000,
    }),
    workerEvent({
      event: "tg.rpc.call",
      requestId: IDS.reqSynthetic,
      method: "channels.inviteToChannel",
      outcome: "started",
      tsMs: nowMs - 20 * MIN,
    }),
    workerEvent({
      event: "tg.flood.wait",
      requestId: IDS.reqSynthetic,
      method: "channels.inviteToChannel",
      outcome: "error",
      waitS: 15_382,
      tsMs: nowMs - 20 * MIN + 5,
    }),
    workerEvent({
      event: "tg.rpc.call",
      requestId: IDS.reqSynthetic,
      method: "channels.inviteToChannel",
      outcome: "aborted",
      err: "worker shutdown during flood wait",
      durationMs: 1000,
      tsMs: nowMs - 20 * MIN + 1005,
    }),

    // Ten days old: outside every 7d window.
    cliEvent({
      runId: "run-old",
      requestId: IDS.reqOld,
      method: "users.getFullUser",
      attempt: 1,
      outcome: "ok",
      tsMs: nowMs - 10 * 24 * HOUR,
    }),
  ]
  return events.map((event) => ({ ...event, eventId: id() }))
}

/** Creates the database at `path` and writes `seedEvents()`. */
export function writeSeedDb(path: string, nowMs: number = NOW_MS): number {
  const db = openTelemetryDb(path)
  const sink = new SqliteTelemetrySink(db, {}, () => {})
  const events = seedEvents(nowMs)
  for (const event of events) sink.record(event)
  const dropped = sink.dropped()
  sink.close()
  if (dropped > 0) throw new Error(`seed dropped ${dropped} events`)
  return events.length
}

/**
 * App log rows for the seeded runs: worker info/warn/error lines on the
 * worker job, one CLI debug line, and one 10-day-old warn line.
 */
export function seedLogEvents(nowMs: number = NOW_MS): TelemetryEvent[] {
  let seq = 0
  const id = (): string => `log-${String(seq++).padStart(4, "0")}`
  const logs: TelemetryEvent[] = [
    cliEvent({
      event: "log.cli",
      tsMs: nowMs - 2 * HOUR + 2000,
      level: "debug",
      msg: "dialogs page read",
      attrs: { count: 100 },
    }),
    workerEvent({
      event: "worker.job.claimed",
      tsMs: nowMs - 30 * MIN + 30_000,
      level: "info",
      msg: "setup claimed",
      attrs: { attempt: 1 },
    }),
    workerEvent({
      event: "worker.job.held",
      tsMs: nowMs - 20 * MIN + 2000,
      level: "warn",
      msg: "setup held for flood wait",
      attrs: { retryInS: 15_382 },
    }),
    workerEvent({
      event: "log.worker",
      jobId: null,
      tsMs: nowMs - 10 * MIN,
      outcome: "error",
      level: "error",
      msg: "worker tick failed",
    }),
    cliEvent({
      runId: "run-old",
      event: "log.cli",
      tsMs: nowMs - 10 * 24 * HOUR + 1,
      level: "warn",
      msg: "old warning",
    }),
  ]
  return logs.map((event) => ({ ...event, eventId: id() }))
}

/** Writes `seedEvents()` plus `seedLogEvents()` into a new database. */
export function writeSeedDbWithLogs(
  path: string,
  nowMs: number = NOW_MS,
): number {
  const count = writeSeedDb(path, nowMs)
  const db = openTelemetryDb(path)
  const sink = new SqliteTelemetrySink(db, {}, () => {})
  const logs = seedLogEvents(nowMs)
  for (const event of logs) sink.record(event)
  const dropped = sink.dropped()
  sink.close()
  if (dropped > 0) throw new Error(`log seed dropped ${dropped} events`)
  return count + logs.length
}
