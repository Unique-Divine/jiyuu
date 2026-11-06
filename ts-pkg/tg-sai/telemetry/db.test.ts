import { Database } from "bun:sqlite"
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  MIGRATIONS,
  NoopSink,
  SqliteTelemetrySink,
  applyMigrations,
  migrationChecksum,
  openTelemetryDb,
  openTelemetrySink,
  telemetryDbPath,
  telemetryDisabled,
  uuidv7,
  type EventRow,
  type TelemetryEvent,
} from "./db"

let dir = ""

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "tg-sai-telemetry-"))
})

afterEach(() => {
  rmSync(dir, { recursive: true, force: true })
})

function sampleEvent(overrides: Partial<TelemetryEvent> = {}): TelemetryEvent {
  return {
    runId: uuidv7(),
    requestId: uuidv7(),
    event: "tg.rpc.attempt",
    subsystem: "worker",
    command: "custody-worker",
    accountId: 8274304175,
    profile: "custody",
    method: "channels.inviteToChannel",
    attempt: 1,
    outcome: "error",
    errorCode: 420,
    err: "FLOOD_WAIT_15382",
    waitS: 15382,
    durationMs: 87,
    req: { _: "channels.inviteToChannel", users: { n: 2, ids: [1, 2] } },
    res: null,
    ...overrides,
  }
}

describe("paths and env", () => {
  test("telemetryDbPath sits in the tg-sai config dir", () => {
    expect(telemetryDbPath({ TG_SAI_DIR: "/var/lib/w" })).toBe(
      "/var/lib/w/telemetry.db",
    )
    expect(telemetryDbPath({ HOME: "/home/u" })).toBe(
      "/home/u/.tg-sai/telemetry.db",
    )
  })

  test("TG_SAI_TELEMETRY=off disables telemetry", () => {
    expect(telemetryDisabled({ TG_SAI_TELEMETRY: "off" })).toBe(true)
    expect(telemetryDisabled({ TG_SAI_TELEMETRY: "OFF " })).toBe(true)
    expect(telemetryDisabled({})).toBe(false)
    const sink = openTelemetrySink(
      { TG_SAI_TELEMETRY: "off", TG_SAI_DIR: dir },
    )
    expect(sink).toBeInstanceOf(NoopSink)
    sink.record(sampleEvent())
    expect(sink.dropped()).toBe(0)
  })
})

describe("uuidv7", () => {
  test("is a version 7 UUID that sorts by time", () => {
    const a = uuidv7(1_700_000_000_000)
    const b = uuidv7(1_700_000_000_001)
    expect(a).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    )
    expect(a < b).toBe(true)
  })
})

describe("migrations", () => {
  test("open sets pragmas and applying twice is idempotent", () => {
    const path = join(dir, "t.db")
    const db = openTelemetryDb(path)
    const mode = db.query<{ journal_mode: string }, []>(
      "PRAGMA journal_mode",
    ).get()
    expect(mode?.journal_mode).toBe("wal")
    const timeout = db.query<{ timeout: number }, []>(
      "PRAGMA busy_timeout",
    ).get()
    expect(timeout?.timeout).toBe(5000)
    const fk = db.query<{ foreign_keys: number }, []>(
      "PRAGMA foreign_keys",
    ).get()
    expect(fk?.foreign_keys).toBe(1)
    expect(applyMigrations(db)).toEqual({
      applied: ["0001-event.sql", "0002-event-job-index.sql"],
      pending: [],
    })
    db.close()

    const again = openTelemetryDb(path)
    const history = again.query<
      { filename: string; checksum: string },
      []
    >("SELECT filename, checksum FROM schema_migrations").all()
    expect(history).toEqual([
      {
        filename: "0001-event.sql",
        checksum: migrationChecksum(MIGRATIONS[0]!.sql),
      },
      {
        filename: "0002-event-job-index.sql",
        checksum: migrationChecksum(MIGRATIONS[1]!.sql),
      },
    ])
    const indexes = again
      .query<{ name: string }, []>(
        "SELECT name FROM sqlite_master WHERE type = 'index' " +
          "AND tbl_name = 'event' AND sql IS NOT NULL ORDER BY name",
      )
      .all()
      .map((row) => row.name)
    expect(indexes).toEqual([
      "event_account_method_event_ts",
      "event_job_id",
      "event_request_id",
      "event_run_id",
      "event_ts_ms",
    ])
    again.close()
  })

  test("creates an owner-only parent dir and database file", () => {
    const path = join(dir, "nested", "telemetry.db")
    openTelemetryDb(path).close()
    expect(statSync(join(dir, "nested")).mode & 0o777).toBe(0o700)
    expect(statSync(path).mode & 0o777).toBe(0o600)
  })

  test("opens an in-memory database without WAL", () => {
    const db = openTelemetryDb(":memory:")
    expect(applyMigrations(db).pending).toEqual([])
    db.close()
  })

  test("refuses an applied migration whose checksum changed", () => {
    const path = join(dir, "t.db")
    openTelemetryDb(path).close()
    const edited = [
      { filename: "0001-event.sql", sql: `${MIGRATIONS[0]!.sql}\n-- edit\n` },
    ]
    expect(() => openTelemetryDb(path, edited)).toThrow(/checksum mismatch/)
  })

  test("refuses an unknown applied migration", () => {
    const path = join(dir, "t.db")
    openTelemetryDb(path).close()
    expect(() => openTelemetryDb(path, [])).toThrow(/Unknown/)
  })

  test("a failing migration rolls back entirely", () => {
    const path = join(dir, "t.db")
    const broken = [
      ...MIGRATIONS,
      { filename: "0003-bad.sql", sql: "CREATE TABLE x (a INT); NOPE;" },
    ]
    expect(() => openTelemetryDb(path, broken)).toThrow()
    const db = new Database(path)
    const tables = db
      .query<{ name: string }, []>(
        "SELECT name FROM sqlite_master WHERE type = 'table'",
      )
      .all()
    expect(tables).toEqual([])
    db.close()
  })

  test("schema enforces outcome and JSON checks", () => {
    const db = openTelemetryDb(join(dir, "t.db"))
    const insert = (outcome: string, req: string | null) =>
      db.run(
        "INSERT INTO event (event_id, run_id, ts_ms, event, subsystem, " +
          "outcome, req) VALUES (?, 'r', 1, 'e', 'cli', ?, ?)",
        [uuidv7(), outcome, req],
      )
    expect(() => insert("ok", '{"_":"x"}')).not.toThrow()
    expect(() => insert("weird", null)).toThrow()
    expect(() => insert("ok", "{not json")).toThrow()
    expect(() =>
      db.run(
        "INSERT INTO event (event_id, run_id, ts_ms, event, subsystem, " +
          "outcome) VALUES (?, 'r', 'soon', 'e', 'cli', 'ok')",
        [uuidv7()],
      ),
    ).toThrow()
    db.close()
  })
})

describe("SqliteTelemetrySink", () => {
  test("insert and query roundtrip", () => {
    const db = openTelemetryDb(join(dir, "t.db"))
    const sink = new SqliteTelemetrySink(db, {
      host: "test-host",
      pid: 4242,
      version: "0.0.6",
    })
    const event = sampleEvent({ eventId: uuidv7(), tsMs: 1_762_400_000_000 })
    sink.record(event)
    expect(sink.dropped()).toBe(0)
    const row = db
      .query<EventRow, [string]>("SELECT * FROM event WHERE event_id = ?")
      .get(event.eventId!)
    expect(row).toEqual({
      event_id: event.eventId!,
      request_id: event.requestId!,
      run_id: event.runId,
      job_id: null,
      ts_ms: 1_762_400_000_000,
      duration_ms: 87,
      event: "tg.rpc.attempt",
      subsystem: "worker",
      command: "custody-worker",
      account_id: 8274304175,
      profile: "custody",
      method: "channels.inviteToChannel",
      attempt: 1,
      outcome: "error",
      error_code: 420,
      err: "FLOOD_WAIT_15382",
      wait_s: 15382,
      req: JSON.stringify(event.req),
      res: null,
      host: "test-host",
      pid: 4242,
      version: "0.0.6",
    })
    sink.close()
  })

  test("fills event_id and ts_ms when omitted", () => {
    const db = openTelemetryDb(join(dir, "t.db"))
    const sink = new SqliteTelemetrySink(db)
    const before = Date.now()
    sink.record({
      runId: "run-1",
      event: "run.start",
      subsystem: "cli",
      outcome: "started",
    })
    const row = db.query<EventRow, []>("SELECT * FROM event").get()
    expect(row?.event_id).toMatch(/^[0-9a-f-]{36}$/)
    expect(row!.ts_ms).toBeGreaterThanOrEqual(before)
    expect(row?.pid).toBe(process.pid)
    sink.close()
  })

  test("record never throws on a closed database and counts drops", () => {
    const db = openTelemetryDb(join(dir, "t.db"))
    const lines: string[] = []
    const sink = new SqliteTelemetrySink(db, {}, (line) => lines.push(line))
    sink.record(sampleEvent())
    sink.close()
    expect(() => sink.record(sampleEvent())).not.toThrow()
    expect(() => sink.record(sampleEvent())).not.toThrow()
    expect(sink.dropped()).toBe(2)
    expect(lines).toHaveLength(2)
    expect(lines[0]).toContain("tg-sai telemetry: dropped tg.rpc.attempt")
  })

  test("record never throws on a read-only database", () => {
    const path = join(dir, "t.db")
    openTelemetryDb(path).close()
    const readonly = new Database(path, { readonly: true })
    const lines: string[] = []
    const sink = new SqliteTelemetrySink(readonly, {}, (l) => lines.push(l))
    expect(() => sink.record(sampleEvent())).not.toThrow()
    expect(sink.dropped()).toBe(1)
    expect(lines[0]).toMatch(/readonly|read-only/i)
    sink.close()
  })

  test("record never throws on unserializable or invalid input", () => {
    const db = openTelemetryDb(join(dir, "t.db"))
    const sink = new SqliteTelemetrySink(db, {}, () => {})
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    expect(() => sink.record(sampleEvent({ req: cyclic }))).not.toThrow()
    expect(() =>
      sink.record(sampleEvent({ outcome: "bogus" as "ok" })),
    ).not.toThrow()
    expect(sink.dropped()).toBe(2)
    sink.close()
  })

  test("openTelemetrySink falls back to a NoopSink when the path is bad", () => {
    const lines: string[] = []
    const blocker = join(dir, "file")
    writeFileSync(blocker, "x")
    const sink = openTelemetrySink(
      {},
      { path: join(blocker, "telemetry.db"), stderr: (l) => lines.push(l) },
    )
    expect(sink).toBeInstanceOf(NoopSink)
    expect(lines).toHaveLength(1)
    expect(() => sink.record(sampleEvent())).not.toThrow()
  })

  test("openTelemetrySink writes to TG_SAI_DIR/telemetry.db", () => {
    const sink = openTelemetrySink({ TG_SAI_DIR: dir }, { version: "t" })
    sink.record(sampleEvent())
    sink.close()
    const db = new Database(join(dir, "telemetry.db"), { readonly: true })
    const row = db.query<{ n: number }, []>(
      "SELECT count(*) AS n FROM event",
    ).get()
    expect(row?.n).toBe(1)
    db.close()
  })
})

describe("concurrency", () => {
  test("four processes open a fresh DB and write concurrently", async () => {
    const path = join(dir, "shared.db")
    const dbModule = join(import.meta.dir, "db.ts")
    const perProcess = 200
    const script = (label: string): string => `
      import { openTelemetryDb, SqliteTelemetrySink } from ${
        JSON.stringify(dbModule)
      }
      const db = openTelemetryDb(${JSON.stringify(path)})
      const sink = new SqliteTelemetrySink(db)
      for (let i = 0; i < ${perProcess}; i++) {
        sink.record({ runId: ${JSON.stringify(label)}, event: "tg.rpc.call",
          subsystem: "cli", outcome: "ok", attempt: i })
      }
      if (sink.dropped() > 0) {
        console.error("dropped", sink.dropped())
        process.exit(1)
      }
      sink.close()
    `
    const labels = ["a", "b", "c", "d"]
    const procs = labels.map((label) =>
      Bun.spawn([process.execPath, "-e", script(label)], {
        stdout: "pipe",
        stderr: "pipe",
      }),
    )
    const codes = await Promise.all(procs.map((p) => p.exited))
    const errs = await Promise.all(
      procs.map((p) => new Response(p.stderr).text()),
    )
    expect(errs).toEqual(labels.map(() => ""))
    expect(codes).toEqual(labels.map(() => 0))

    const db = new Database(path, { readonly: true })
    const counts = db
      .query<{ run_id: string; n: number }, []>(
        "SELECT run_id, count(*) AS n FROM event GROUP BY run_id " +
          "ORDER BY run_id",
      )
      .all()
    expect(counts).toEqual(
      labels.map((label) => ({ run_id: label, n: perProcess })),
    )
    const migrations = db.query<{ n: number }, []>(
      "SELECT count(*) AS n FROM schema_migrations",
    ).get()
    expect(migrations?.n).toBe(MIGRATIONS.length)
    db.close()
  }, 30_000)
})
