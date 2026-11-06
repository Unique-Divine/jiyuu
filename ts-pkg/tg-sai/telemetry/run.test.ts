import { describe, expect, test } from "bun:test"
import { existsSync, mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { MtPeerNotFoundError, tl } from "@mtcute/bun"

import {
  NoopSink,
  openTelemetryDb,
  type EventRow,
  type TelemetryEvent,
  type TelemetrySink,
} from "./db"
import {
  activeTelemetryRun,
  BOOTSTRAP_FLOOD_MAX_WAIT_MS,
  cleanLogMsg,
  floodMaxWaitMs,
  log,
  LOG_MSG_MAX_CHARS,
  logEventFor,
  logEventName,
  REDACTED,
  sanitizeArgv,
  startTelemetryRun,
} from "./run"

function withDir<T>(fn: (dir: string) => T): T {
  const dir = mkdtempSync(join(tmpdir(), "tg-sai-run-"))
  try {
    return fn(dir)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

/** Keeps recorded events in memory, in order. */
class ArraySink implements TelemetrySink {
  readonly events: TelemetryEvent[] = []
  closed = false
  record(event: TelemetryEvent): void {
    this.events.push(event)
  }
  dropped(): number {
    return 0
  }
  close(): void {
    this.closed = true
  }
}

const CONTEXT = {
  runId: "run-1",
  subsystem: "worker" as const,
  command: "worker",
  version: "0.0.0-test",
  accountId: 8274304175,
  profile: "custody",
}

describe("startTelemetryRun", () => {
  test("records run.start/run.end, writes no log files, clears the run",
    () => {
      withDir((dir) => {
        const sink = new ArraySink()
        const run = startTelemetryRun({
          subsystem: "worker",
          command: "worker",
          argv: ["worker", "--custody-profile", "custody", "--run"],
          version: "0.0.0-test",
          env: { TG_SAI_DIR: dir },
          sink,
        })
        expect(activeTelemetryRun()).toBe(run)
        expect(run.floodMaxWaitMs).toBe(30000)
        run.log("info", "worker.tick", "tick done", { jobs: 0 })
        run.markAborted()
        run.end({ exitCode: 0 })
        run.end({ exitCode: 0 })
        run.log("info", "worker.tick", "after end")
        expect(activeTelemetryRun()).toBeNull()

        expect(sink.events.map((e) => e.event)).toEqual([
          "run.start",
          "worker.tick",
          "run.end",
        ])
        expect(
          sink.events.every((e) => e.runId === run.context.runId),
        ).toBe(true)
        expect(sink.events[2]).toMatchObject({
          outcome: "aborted",
          errorCode: 0,
        })
        expect(sink.closed).toBe(true)
        expect(existsSync(join(dir, "logs"))).toBe(false)
      })
    })

  test("failed runs record the exit code and error", () => {
    const sink = new ArraySink()
    const run = startTelemetryRun({
      subsystem: "cli",
      command: "ops sai-find",
      argv: [],
      version: "0.0.0-test",
      env: {},
      sink,
    })
    run.end({ exitCode: 1, error: new Error("FLOOD_WAIT_15382") })
    expect(sink.events.at(-1)).toMatchObject({
      event: "run.end",
      outcome: "error",
      errorCode: 1,
      err: "Error [FLOOD_WAIT_15382]",
    })
  })

  test("run.end keeps free-form error messages out of the row", () => {
    const cases: [unknown, string][] = [
      [
        new Error(
          "Missing chatId in JSONL input line: " +
            '{"title":"x","link":"t.me/+abc"}',
        ),
        "Error",
      ],
      [new MtPeerNotFoundError("Peer @alice not found"), "MtPeerNotFoundError"],
      [new Error("Unknown profile: x"), "Error"],
      [
        tl.RpcError.fromTl({
          _: "mt_rpc_error",
          errorCode: 420,
          errorMessage: "FLOOD_WAIT_400",
        }),
        "RpcError [420 FLOOD_WAIT_400]",
      ],
    ]
    for (const [error, err] of cases) {
      const sink = new ArraySink()
      const run = startTelemetryRun({
        subsystem: "cli",
        command: "ops sai-bootstrap",
        argv: [],
        version: "0.0.0-test",
        env: {},
        sink,
      })
      run.end({ exitCode: 1, error })
      expect(sink.events.at(-1)?.err).toBe(err)
    }
  })

  test("floodMaxWaitMs option sets the default; the env var still wins",
    () => {
      const run = startTelemetryRun({
        subsystem: "cli",
        command: "ops sai-bootstrap",
        argv: [],
        version: "0.0.0-test",
        env: {},
        floodMaxWaitMs: BOOTSTRAP_FLOOD_MAX_WAIT_MS,
        sink: new NoopSink(),
      })
      expect(run.floodMaxWaitMs).toBe(300_000)
      expect(run.client().floodWaiter).toEqual({ maxWait: 300_000 })
      run.end({ exitCode: 0 })
      expect(
        floodMaxWaitMs("cli", { TG_SAI_FLOOD_MAX_WAIT_MS: "5000" }, 300_000),
      ).toBe(5000)
      expect(floodMaxWaitMs("cli", {}, 300_000)).toBe(300_000)
    })
})

describe("log rows", () => {
  test("logEventFor builds a level/msg/attrs row with run context", () => {
    const event = logEventFor(
      CONTEXT,
      "warn",
      "worker.job.held",
      "setup held for flood wait",
      { retryInS: 60, chatTitle: "Deal with Alice", sessionString: "s3cr3t" },
      { jobId: "job-1", tsMs: 5 },
    )
    expect(event).toMatchObject({
      runId: "run-1",
      subsystem: "worker",
      command: "worker",
      accountId: 8274304175,
      profile: "custody",
      jobId: "job-1",
      tsMs: 5,
      event: "worker.job.held",
      outcome: "ok",
      level: "warn",
      msg: "setup held for flood wait",
    })
    // Strings become lengths; secret keys are dropped.
    expect(event.attrs).toEqual({ retryInS: 60, chatTitle: { len: 15 } })
    expect(JSON.stringify(event)).not.toContain("Alice")
    expect(JSON.stringify(event)).not.toContain("s3cr3t")
  })

  test("error level maps to the error outcome", () => {
    expect(logEventFor(CONTEXT, "error", "log.worker", "tick failed"))
      .toMatchObject({ outcome: "error", level: "error", attrs: null })
  })

  test("reserved or malformed event names fall back to log.<subsystem>",
    () => {
      expect(logEventName("worker.job.claimed", "worker"))
        .toBe("worker.job.claimed")
      expect(logEventName("log.worker", "worker")).toBe("log.worker")
      expect(logEventName("tg.rpc.call", "cli")).toBe("log.cli")
      expect(logEventName("run.end", "cli")).toBe("log.cli")
      expect(logEventName("Has Spaces", "health")).toBe("log.health")
      expect(logEventName("single", "cli")).toBe("log.cli")
    })

  test("cleanLogMsg strips control characters and caps length", () => {
    expect(cleanLogMsg("a\nb\tc")).toBe("a b c")
    expect(cleanLogMsg("x".repeat(500))).toHaveLength(LOG_MSG_MAX_CHARS)
  })

  test("standalone log writes one row to TG_SAI_DIR/telemetry.db", () => {
    withDir((dir) => {
      log("info", "log.cli", "profile saved", { count: 2 }, undefined, {
        env: { TG_SAI_DIR: dir },
        version: "0.0.0-test",
      })
      const db = openTelemetryDb(join(dir, "telemetry.db"))
      const rows = db.query<EventRow, []>("SELECT * FROM event").all()
      db.close()
      expect(rows).toHaveLength(1)
      expect(rows[0]).toMatchObject({
        event: "log.cli",
        subsystem: "cli",
        outcome: "ok",
        level: "info",
        msg: "profile saved",
        attrs: '{"count":2}',
      })
      expect(existsSync(join(dir, "logs"))).toBe(false)
    })
  })

  test("standalone log honors TG_SAI_TELEMETRY=off", () => {
    withDir((dir) => {
      log("info", "log.cli", "ignored", undefined, undefined, {
        env: { TG_SAI_DIR: dir, TG_SAI_TELEMETRY: "off" },
      })
      expect(existsSync(join(dir, "telemetry.db"))).toBe(false)
    })
  })
})

describe("sanitizeArgv", () => {
  const R = REDACTED
  const cases: [string, string[], string[]][] = [
    [
      "password value starting with a dash",
      ["profile", "add", "--password", "-hunter2", "--name", "custody"],
      ["profile", "add", "--password", R, "--name", R],
    ],
    [
      "target user handle",
      ["ops", "sai-bootstrap", "--target-user", "@alice", "--run"],
      ["ops", "sai-bootstrap", "--target-user", R, "--run"],
    ],
    [
      "free-text reason",
      ["provision", "remove", "--reason", "free text", "--json"],
      ["provision", "remove", "--reason", R, "--json"],
    ],
    [
      "positional profile selector",
      ["profile", "use", "alice"],
      ["profile", "use", R],
    ],
    [
      "file keeps only its basename",
      ["ops", "sai-bootstrap", "--file", "/home/x/chats-00.jsonl"],
      ["ops", "sai-bootstrap", "--file", "chats-00.jsonl"],
    ],
    [
      "numeric chat kept, username chat redacted",
      ["audit", "--chat", "-100123", "--chat", "@group"],
      ["audit", "--chat", "-100123", "--chat", R],
    ],
    [
      "numeric safe flags kept, odd shapes redacted",
      ["worker", "--limit", "10", "--interval-ms", "5000", "--limit", "x"],
      ["worker", "--limit", "10", "--interval-ms", "5000", "--limit", R],
    ],
    [
      "since durations kept",
      ["events", "tail", "--since", "15m", "--dry-run"],
      ["events", "tail", "--since", "15m", "--dry-run"],
    ],
    [
      "= forms follow the same rules",
      ["audit", "--chat=-100123", "--api-hash=abc", "--file=/a/b.json"],
      ["audit", "--chat=-100123", `--api-hash=${R}`, "--file=b.json"],
    ],
    [
      "unknown flag keeps its name; next positional redacted",
      ["audit", "--mystery", "value", "--json"],
      ["audit", "--mystery", R, "--json"],
    ],
    [
      "everything after -- is redacted",
      ["health", "--", "--limit", "5"],
      ["health", "--", R, R],
    ],
    [
      "command words after a value are redacted",
      ["profile", "show", "--profile", "x", "list"],
      ["profile", "show", "--profile", R, R],
    ],
    [
      "trailing value flag without a value",
      ["profile", "add", "--password"],
      ["profile", "add", "--password"],
    ],
  ]
  for (const [name, argv, expected] of cases) {
    test(name, () => {
      expect(sanitizeArgv(argv)).toEqual(expected)
    })
  }
})
