import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import { Command } from "commander"
import { existsSync, mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  isoTime,
  parseAccountId,
  parseDuration,
  parseLayer,
  parsePositiveInt,
  registerEventsCommand,
  sinceCutoff,
  toonObject,
  toonTable,
  toonValue,
} from "./cli.ts"
import { SqliteTelemetrySink, openTelemetryDb, telemetryDbPath } from "./db.ts"
import {
  CUSTODY_ACCOUNT,
  IDS,
  NOW_MS,
  seedEvents,
  writeSeedDb,
} from "./testEvents.ts"

let dir = ""
let dbPath = ""

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "tg-sai-events-cli-"))
  dbPath = join(dir, "telemetry.db")
  writeSeedDb(dbPath)
})

afterAll(() => {
  rmSync(dir, { recursive: true, force: true })
})

interface RunResult {
  out: string[]
  err: string[]
}

async function runEvents(
  args: string[],
  path: string = dbPath,
): Promise<RunResult> {
  const out: string[] = []
  const err: string[] = []
  const program = new Command().name("tg-sai").exitOverride()
  registerEventsCommand(program, {
    dbPath: path,
    now: () => NOW_MS,
    stdout: (text) => out.push(text),
    stderr: (text) => err.push(text),
  })
  await program.parseAsync(["events", ...args], { from: "user" })
  return { out, err }
}

describe("parseDuration", () => {
  test("parses each unit", () => {
    expect(parseDuration("500ms")).toBe(500)
    expect(parseDuration("90s")).toBe(90_000)
    expect(parseDuration("30m")).toBe(1_800_000)
    expect(parseDuration("24h")).toBe(86_400_000)
    expect(parseDuration("7d")).toBe(604_800_000)
    expect(parseDuration("2w")).toBe(1_209_600_000)
    expect(parseDuration("1.5h")).toBe(5_400_000)
    expect(parseDuration(" 7d ")).toBe(604_800_000)
  })

  test("rejects malformed and non-positive durations", () => {
    for (const bad of ["", "7", "d", "7 d", "-1h", "1y", "1h30m", "0s"]) {
      expect(() => parseDuration(bad)).toThrow()
    }
  })

  test("sinceCutoff subtracts from now", () => {
    expect(sinceCutoff("1h", NOW_MS)).toBe(NOW_MS - 3_600_000)
  })
})

describe("option parsers", () => {
  test("validate integers, accounts, and layers", () => {
    expect(parsePositiveInt("20", "--limit")).toBe(20)
    expect(() => parsePositiveInt("0", "--limit")).toThrow(/--limit/)
    expect(() => parsePositiveInt("2.5", "--limit")).toThrow()
    expect(parseAccountId("8274304175")).toBe(8274304175)
    expect(() => parseAccountId("@SaiTeam_AI")).toThrow(/--account/)
    expect(parseLayer("call")).toBe("call")
    expect(() => parseLayer("rows")).toThrow(/--layer/)
  })
})

describe("TOON encoding", () => {
  test("leaves safe strings bare and quotes ambiguous ones", () => {
    expect(toonValue("messages.getDialogs")).toBe("messages.getDialogs")
    expect(toonValue("FLOOD_WAIT_3")).toBe("FLOOD_WAIT_3")
    expect(toonValue(null)).toBe("null")
    expect(toonValue(undefined)).toBe("null")
    expect(toonValue(42)).toBe("42")
    expect(toonValue(true)).toBe("true")
    expect(toonValue("a,b")).toBe('"a,b"')
    expect(toonValue('say "hi"')).toBe('"say \\"hi\\""')
    expect(toonValue("[x]")).toBe('"[x]"')
    expect(toonValue("{x}")).toBe('"{x}"')
    expect(toonValue("2026-11-06T12:00:00.000Z")).toBe(
      '"2026-11-06T12:00:00.000Z"',
    )
    expect(toonValue("")).toBe('""')
    expect(toonValue(" pad")).toBe('" pad"')
    expect(toonValue("true")).toBe('"true"')
    expect(toonValue("null")).toBe('"null"')
    expect(toonValue("123")).toBe('"123"')
    expect(toonValue("-x")).toBe('"-x"')
    expect(toonValue("line\nbreak")).toBe('"line\\nbreak"')
    expect(toonValue("back\\slash")).toBe('"back\\\\slash"')
    expect(toonValue({ _: "x", n: 2 })).toBe('"{\\"_\\":\\"x\\",\\"n\\":2}"')
  })

  test("tables declare row count and fields", () => {
    expect(
      toonTable(
        "files",
        ["key", "size"],
        [
          { key: "a.pdf", size: 1 },
          { key: "b,c.png", size: 2 },
        ],
      ),
    ).toBe('files[2]{key,size}:\n  a.pdf,1\n  "b,c.png",2')
    expect(toonTable("files", ["key"], [])).toBe("files[0]:")
  })

  test("objects are key: value lines", () => {
    expect(toonObject({ rows: 3, path: "/a/b" })).toBe("rows: 3\npath: /a/b")
  })
})

describe("missing database", () => {
  test("prints a clear note and succeeds", async () => {
    const missing = join(dir, "absent", "telemetry.db")
    for (const sub of [["tail"], ["stats"], ["export"], ["runs"]]) {
      const result = await runEvents(sub, missing)
      expect(result.out).toEqual([])
      expect(result.err[0]).toContain("No telemetry database")
      expect(result.err[0]).toContain(missing)
    }
    expect(existsSync(missing)).toBe(false)
  })

  test("default path follows TG_SAI_DIR", () => {
    expect(telemetryDbPath({ TG_SAI_DIR: dir })).toBe(dbPath)
  })
})

describe("events tail", () => {
  test("TOON table, newest first", async () => {
    const { out } = await runEvents(["tail", "--limit", "3"])
    expect(out).toHaveLength(1)
    const lines = (out[0] ?? "").split("\n")
    expect(lines[0]).toBe(
      "events[3]{ts,event,outcome,account_id,method,attempt,duration_ms," +
        "error_code,err,wait_s,request_id}:",
    )
    expect(lines[1]).toBe(
      `  "${isoTime(NOW_MS - 20 * 60_000 + 1005)}",tg.rpc.call,aborted,` +
        `${CUSTODY_ACCOUNT},channels.inviteToChannel,null,1000,null,` +
        `worker shutdown during flood wait,null,${IDS.reqSynthetic}`,
    )
    expect(lines).toHaveLength(4)
  })

  test("--json with filters", async () => {
    const { out } = await runEvents([
      "tail",
      "--errors",
      "--account",
      String(CUSTODY_ACCOUNT),
      "--since",
      "1h",
      "--json",
    ])
    const rows = JSON.parse(out[0] ?? "[]") as Record<string, unknown>[]
    expect(rows.map((r) => [r.event, r.outcome])).toEqual([
      ["tg.rpc.call", "aborted"],
      ["tg.flood.wait", "error"],
      ["tg.rpc.attempt", "timeout"],
    ])
  })

  test("--run and --method narrow rows", async () => {
    const { out } = await runEvents([
      "tail",
      "--run",
      IDS.runCli,
      "--method",
      "channels.editAdmin",
      "--json",
    ])
    const rows = JSON.parse(out[0] ?? "[]") as Record<string, unknown>[]
    expect(rows).toHaveLength(2)
  })

  test("invalid options throw", async () => {
    await expect(runEvents(["tail", "--since", "soon"])).rejects.toThrow(
      /Invalid duration/,
    )
    await expect(runEvents(["tail", "--limit", "0"])).rejects.toThrow()
  })
})

describe("events show", () => {
  test("request timeline with parsed req/res in JSON", async () => {
    const { out } = await runEvents(["show", IDS.reqFlooded, "--json"])
    const rows = JSON.parse(out[0] ?? "[]") as Record<string, unknown>[]
    expect(rows.map((r) => r.event)).toEqual([
      "tg.rpc.call",
      "tg.rpc.attempt",
      "tg.flood.wait",
      "tg.rpc.attempt",
      "tg.rpc.call",
    ])
    expect(rows[0]?.req).toEqual({ _: "messages.getDialogs", limit: 100 })
    expect(rows[3]?.res).toEqual({ _: "messages.dialogsSlice", count: 412 })
  })

  test("TOON quotes JSON cells", async () => {
    const { out } = await runEvents(["show", IDS.reqFlooded])
    const text = out[0] ?? ""
    expect(text.startsWith("timeline[5]{")).toBe(true)
    expect(text).toContain(
      '"{\\"_\\":\\"messages.getDialogs\\",\\"limit\\":100}"',
    )
  })

  test("run id shows the whole run; unknown id notes it", async () => {
    const run = await runEvents(["show", IDS.runCli])
    expect(run.out[0]?.startsWith("timeline[10]{")).toBe(true)
    const none = await runEvents(["show", "missing-id"])
    expect(none.out).toEqual(["timeline[0]:"])
    expect(none.err[0]).toContain("missing-id")
  })
})

describe("events counts", () => {
  test("defaults to attempts by method,outcome over 24h", async () => {
    const { out } = await runEvents(["counts"])
    const lines = (out[0] ?? "").split("\n")
    expect(lines[0]).toBe("counts[4]{method,outcome,count}:")
    expect(lines).toContain("  messages.getDialogs,error,1")
    expect(lines).toContain("  channels.createChannel,timeout,1")
  })

  test("call layer by outcome for one account", async () => {
    const { out } = await runEvents([
      "counts",
      "--layer",
      "call",
      "--by",
      "outcome",
      "--account",
      String(CUSTODY_ACCOUNT),
      "--json",
    ])
    expect(JSON.parse(out[0] ?? "[]")).toEqual([
      { outcome: "aborted", count: 1 },
    ])
  })

  test("rejects unknown fields and layers", async () => {
    await expect(runEvents(["counts", "--by", "secret"])).rejects.toThrow(
      /Unknown --by/,
    )
    await expect(runEvents(["counts", "--layer", "x"])).rejects.toThrow()
  })
})

describe("events floods", () => {
  test("lists waits and the active synthetic flood with its end", async () => {
    const { out } = await runEvents(["floods"])
    expect(out).toHaveLength(2)
    expect(out[0]).toContain(
      "floods[3]{event,account_id,method,wait_s,count,latest}:",
    )
    const ends = NOW_MS - 20 * 60_000 + 5 + 15_382_000
    const remaining = Math.ceil((ends - NOW_MS) / 1000)
    expect(out[1]).toBe(
      "active[1]{account_id,method,wait_s,started,ends,remaining_s," +
        "request_id}:\n" +
        `  ${CUSTODY_ACCOUNT},channels.inviteToChannel,15382,` +
        `"${isoTime(NOW_MS - 20 * 60_000 + 5)}","${isoTime(ends)}",` +
        `${remaining},${IDS.reqSynthetic}`,
    )
  })

  test("--json has floods and active", async () => {
    const { out } = await runEvents(["floods", "--json"])
    const parsed = JSON.parse(out[0] ?? "{}") as {
      floods: unknown[]
      active: { method: string }[]
    }
    expect(parsed.floods).toHaveLength(3)
    expect(parsed.active.map((a) => a.method)).toEqual([
      "channels.inviteToChannel",
    ])
  })
})

describe("events uncertain", () => {
  test("lists the call with no completion", async () => {
    const { out } = await runEvents(["uncertain", "--json"])
    const rows = JSON.parse(out[0] ?? "[]") as Record<string, unknown>[]
    expect(rows).toEqual([
      {
        ts: isoTime(NOW_MS - 29 * 60_000),
        age_s: 29 * 60,
        account_id: CUSTODY_ACCOUNT,
        method: "channels.createChannel",
        command: "custody-worker",
        run_id: IDS.runWorker,
        request_id: IDS.reqUncertain,
      },
    ])
  })
})

describe("events runs", () => {
  test("pairs starts and ends, marking unfinished runs", async () => {
    const { out } = await runEvents(["runs"])
    const lines = (out[0] ?? "").split("\n")
    expect(lines[0]).toBe(
      "runs[2]{started,run_id,command,subsystem,outcome,error_code,err," +
        "duration_ms,calls,errors,attempts,ticks}:",
    )
    expect(lines[1]).toContain(
      `${IDS.runWorker},custody-worker,worker,unfinished,null,null,null,1,1,1,1`,
    )
    expect(lines[2]).toContain(
      `${IDS.runCli},ops sai-find,cli,ok,null,null,10000,2,1,3,0`,
    )
    const limited = await runEvents(["runs", "--limit", "1", "--json"])
    expect(JSON.parse(limited.out[0] ?? "[]")).toHaveLength(1)
  })
})

describe("events stats", () => {
  test("reports path, size, rows, range, and kinds", async () => {
    const { out } = await runEvents(["stats", "--json"])
    const stats = JSON.parse(out[0] ?? "{}") as Record<string, unknown>
    expect(stats.path).toBe(dbPath)
    expect(stats.size_bytes as number).toBeGreaterThan(0)
    expect(typeof stats.wal_bytes).toBe("number")
    expect(stats.rows).toBe(seedEvents().length)
    expect(stats.oldest).toBe(isoTime(NOW_MS - 10 * 86_400_000))
    expect(stats.kinds).toContainEqual({ event: "tg.rpc.call", rows: 7 })
  })

  test("TOON has summary lines then a kinds table", async () => {
    const { out } = await runEvents(["stats"])
    expect(out[0]).toContain(`path: ${dbPath}`)
    expect(out[0]).toContain(`rows: ${seedEvents().length}`)
    expect(out[1]?.startsWith("kinds[6]{event,rows}:")).toBe(true)
  })
})

describe("events export", () => {
  test("writes JSONL oldest first within --since", async () => {
    const week = await runEvents(["export"])
    const rows = week.out.map(
      (line) => JSON.parse(line) as Record<string, unknown>,
    )
    expect(rows).toHaveLength(seedEvents().length - 1)
    const ts = rows.map((r) => r.ts_ms as number)
    expect(ts).toEqual([...ts].sort((a, b) => a - b))
    const first = rows.find((r) => r.request_id === IDS.reqFlooded)
    expect(first?.req).toEqual({ _: "messages.getDialogs", limit: 100 })
    const all = await runEvents(["export", "--since", "30d"])
    expect(all.out).toHaveLength(seedEvents().length)
  })
})

describe("reading while a writer is open", () => {
  test("sees rows still in the WAL", async () => {
    const livePath = join(dir, "live.db")
    writeSeedDb(livePath)
    const writer = openTelemetryDb(livePath)
    const sink = new SqliteTelemetrySink(writer, {}, () => {})
    sink.record({
      runId: "run-live",
      requestId: "req-live",
      event: "tg.rpc.attempt",
      subsystem: "cli",
      method: "help.getConfig",
      attempt: 1,
      outcome: "ok",
      tsMs: NOW_MS - 1000,
    })
    try {
      expect(existsSync(`${livePath}-wal`)).toBe(true)
      const { out } = await runEvents(["show", "req-live", "--json"], livePath)
      expect(JSON.parse(out[0] ?? "[]")).toHaveLength(1)
    } finally {
      sink.close()
    }
  })
})
