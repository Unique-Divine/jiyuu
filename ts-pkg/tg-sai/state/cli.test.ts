import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { Command, CommanderError } from "commander"
import { existsSync, mkdtempSync, rmSync, statSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  defaultMethod,
  holdProjection,
  manualRequestId,
  parseHoldGroup,
  parseReasonCode,
  parseUntil,
  plannedUntil,
  foreignOwnerRefusal,
  registerHoldsCommand,
  userNameOf,
  type HoldsCliContext,
} from "./cli.ts"
import { openStateDb, stateDbPath } from "./db.ts"
import { HoldStore, MAX_HOLD_MS, holdUntil, type FloodHold } from "./holds.ts"
import {
  openTelemetryDb,
  telemetryDbPath,
  type EventRow,
} from "../telemetry/db.ts"

const NOW_MS = Date.parse("2026-11-06T12:00:00.000Z")
const ACCOUNT = "8274304175"

let dir = ""
let env: Record<string, string> = {}

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "tg-sai-holds-cli-"))
  env = { TG_SAI_DIR: dir }
})

afterEach(() => {
  rmSync(dir, { recursive: true, force: true })
})

interface RunResult {
  out: string[]
  err: string[]
  exitCode: number | null
}

async function runHolds(
  args: string[],
  extra: Partial<HoldsCliContext> = {},
): Promise<RunResult> {
  const out: string[] = []
  const err: string[] = []
  const program = new Command().name("tg-sai").exitOverride()
  registerHoldsCommand(program, {
    env: { ...env, TG_SAI_TELEMETRY: "on" },
    now: () => NOW_MS,
    stdout: (text) => out.push(text),
    stderr: (text) => err.push(text),
    ...extra,
  })
  try {
    await program.parseAsync(["holds", ...args], { from: "user" })
    return { out, err, exitCode: null }
  } catch (error) {
    if (error instanceof CommanderError) {
      return { out, err, exitCode: error.exitCode }
    }
    throw error
  }
}

function seed(holds: Parameters<HoldStore["recordHold"]>[0][]): void {
  const store = HoldStore.fromDb(openStateDb(stateDbPath(env)))
  try {
    for (const hold of holds) store.recordHold(hold)
  } finally {
    store.close()
  }
}

function readHolds(): FloodHold[] {
  const store = HoldStore.fromDb(openStateDb(stateDbPath(env)))
  try {
    return store.activeHolds(null, NOW_MS)
  } finally {
    store.close()
  }
}

describe("parsers", () => {
  test("parseHoldGroup accepts named groups and method:<name>", () => {
    for (const g of ["invite", "create", "resolve", "send"]) {
      expect(parseHoldGroup(g)).toBe(g as never)
    }
    expect(parseHoldGroup("method:users.getFullUser")).toBe(
      "method:users.getFullUser",
    )
    for (const bad of ["", "INVITE", "method:", "method:foo", "other"]) {
      expect(() => parseHoldGroup(bad)).toThrow()
    }
  })

  test("parseUntil needs a zone and a future time", () => {
    expect(parseUntil("2026-11-07T00:00:00Z", NOW_MS)).toBe(
      Date.parse("2026-11-07T00:00:00Z"),
    )
    expect(parseUntil("2026-11-07T01:00:00+01:00", NOW_MS)).toBe(
      Date.parse("2026-11-07T00:00:00Z"),
    )
    expect(() => parseUntil("2026-11-07T00:00:00", NOW_MS)).toThrow()
    expect(() => parseUntil("tomorrow", NOW_MS)).toThrow()
    expect(() => parseUntil("2026-11-06T11:00:00Z", NOW_MS)).toThrow()
  })

  test("reason codes, default methods, and request ids", () => {
    expect(parseReasonCode("ops_pause-1")).toBe("ops_pause-1")
    expect(() => parseReasonCode("has space")).toThrow()
    expect(defaultMethod("invite")).toBe("manual")
    expect(defaultMethod("method:users.getFullUser")).toBe("users.getFullUser")
    expect(manualRequestId(undefined)).toBeNull()
    expect(manualRequestId("ops")).toBe("manual:ops")
  })

  test("plannedUntil keeps the later end and caps at one year", () => {
    const existing = {
      untilMs: NOW_MS + 60_000,
    } as FloodHold
    expect(plannedUntil(NOW_MS + 1000, existing, NOW_MS)).toEqual({
      untilMs: NOW_MS + 60_000,
      extends: false,
    })
    expect(plannedUntil(NOW_MS + 120_000, existing, NOW_MS)).toEqual({
      untilMs: NOW_MS + 120_000,
      extends: true,
    })
    expect(plannedUntil(NOW_MS + 2 * MAX_HOLD_MS, null, NOW_MS)).toEqual({
      untilMs: NOW_MS + MAX_HOLD_MS,
      extends: true,
    })
  })

  test("holdProjection reports 0 remaining once a hold ended", () => {
    const hold: FloodHold = {
      accountId: ACCOUNT,
      grp: "invite",
      untilMs: NOW_MS - 1,
      method: "channels.inviteToChannel",
      waitS: 30,
      source: "flood_wait",
      requestId: null,
      createdMs: NOW_MS - 40_000,
      updatedMs: NOW_MS - 40_000,
    }
    expect(holdProjection(hold, NOW_MS).remaining_s).toBe(0)
    expect(holdProjection(hold, NOW_MS - 30_000).remaining_s).toBe(30)
  })
})

describe("holds help", () => {
  test("bare group prints help and exits 0", async () => {
    const result = await runHolds([])
    expect(result.exitCode).toBeNull()
    expect(result.out.join("\n")).toContain("Usage: tg-sai holds")
    expect(result.out.join("\n")).toContain("holds clear --account")
  })

  test("bare set and clear print their help", async () => {
    for (const sub of ["set", "clear"]) {
      const result = await runHolds([sub])
      expect(result.exitCode).toBeNull()
      expect(result.out.join("\n")).toContain(`Usage: tg-sai holds ${sub}`)
    }
    expect(existsSync(stateDbPath(env))).toBe(false)
  })

  test("--help on each subcommand exits 0", async () => {
    for (const sub of ["list", "set", "clear"]) {
      const result = await runHolds([sub, "--help"])
      expect(result.exitCode).toBe(0)
      expect(result.out.join("\n")).toContain(`Usage: tg-sai holds ${sub}`)
    }
  })
})

describe("holds list", () => {
  test("no database prints an empty table without creating one", async () => {
    const result = await runHolds(["list"])
    expect(result.out).toEqual(["holds[0]:"])
    expect(existsSync(stateDbPath(env))).toBe(false)
  })

  test("active holds as TOON and JSON; --all adds ended ones", async () => {
    seed([
      {
        accountId: ACCOUNT,
        grp: "invite",
        method: "channels.inviteToChannel",
        source: "flood_wait",
        waitS: 600,
        nowMs: NOW_MS,
      },
      {
        accountId: "42",
        grp: "send",
        method: "messages.sendMessage",
        source: "peer_flood",
        untilMs: NOW_MS - 1000,
        waitS: 86400,
        nowMs: NOW_MS - 90_000_000,
      },
    ])
    const until = holdUntil(NOW_MS, 600)

    const toon = await runHolds(["list"])
    expect(toon.out).toEqual([
      [
        "holds[1]{account,group,until,remaining_s,method,wait_s,source}:",
        `  "${ACCOUNT}",invite,"${new Date(until).toISOString()}",` +
          `${Math.ceil((until - NOW_MS) / 1000)},` +
          "channels.inviteToChannel,600,flood_wait",
      ].join("\n"),
    ])

    const json = await runHolds(["list", "--json"])
    expect(JSON.parse(json.out[0]!)).toEqual([
      {
        account: ACCOUNT,
        group: "invite",
        until: new Date(until).toISOString(),
        remaining_s: Math.ceil((until - NOW_MS) / 1000),
        method: "channels.inviteToChannel",
        wait_s: 600,
        source: "flood_wait",
      },
    ])

    const all = await runHolds(["list", "--all", "--json"])
    const rows = JSON.parse(all.out[0]!) as { account: string }[]
    expect(rows.map((r) => r.account)).toEqual([ACCOUNT, "42"])

    const one = await runHolds(["list", "--all", "--account", "42", "--json"])
    expect(JSON.parse(one.out[0]!)).toHaveLength(1)
  })

  test("rejects a non-numeric account", async () => {
    const result = await runHolds(["list", "--account", "abc"])
    expect(result.exitCode).toBe(1)
    expect(result.err.join("\n")).toContain("--account")
  })
})

describe("holds set", () => {
  test("dry run reports the hold and writes nothing", async () => {
    const result = await runHolds([
      "set",
      "--account",
      ACCOUNT,
      "--group",
      "invite",
      "--wait-s",
      "3600",
      "--json",
    ])
    const plan = JSON.parse(result.out[0]!)
    expect(plan.mode).toBe("dry_run")
    expect(plan.until).toBe(new Date(holdUntil(NOW_MS, 3600)).toISOString())
    expect(plan.changed).toBe(true)
    expect(plan.source).toBe("manual")
    expect(result.err.join("\n")).toContain("Dry run")
    expect(existsSync(stateDbPath(env))).toBe(false)
  })

  test("--run writes a manual hold with the reason code", async () => {
    const result = await runHolds([
      "set",
      "--account",
      ACCOUNT,
      "--group",
      "send",
      "--until",
      "2026-11-07T00:00:00Z",
      "--reason-code",
      "ops_pause",
      "--run",
    ])
    expect(result.out[0]).toContain("mode: run")
    expect(result.out[0]).toContain('until: "2026-11-07T00:00:00.000Z"')
    const [hold] = readHolds()
    expect(hold).toMatchObject({
      accountId: ACCOUNT,
      grp: "send",
      untilMs: Date.parse("2026-11-07T00:00:00Z"),
      method: "manual",
      waitS: null,
      source: "manual",
      requestId: "manual:ops_pause",
    })
  })

  test("a shorter manual hold leaves a later one unchanged", async () => {
    seed([
      {
        accountId: ACCOUNT,
        grp: "invite",
        method: "channels.inviteToChannel",
        source: "flood_wait",
        waitS: 7200,
        nowMs: NOW_MS,
      },
    ])
    const result = await runHolds([
      "set",
      "--account",
      ACCOUNT,
      "--group",
      "invite",
      "--wait-s",
      "60",
      "--run",
      "--json",
    ])
    const written = JSON.parse(result.out[0]!)
    expect(written.changed).toBe(false)
    expect(written.source).toBe("flood_wait")
    expect(written.until).toBe(new Date(holdUntil(NOW_MS, 7200)).toISOString())
    expect(readHolds()[0]).toMatchObject({
      source: "flood_wait",
      untilMs: holdUntil(NOW_MS, 7200),
    })
  })

  test("method groups default the method to the group name", async () => {
    await runHolds([
      "set",
      "--account",
      ACCOUNT,
      "--group",
      "method:users.getFullUser",
      "--wait-s",
      "30",
      "--run",
    ])
    expect(readHolds()[0]?.method).toBe("users.getFullUser")
  })

  test("usage errors exit 1", async () => {
    const cases: string[][] = [
      ["set", "--account", ACCOUNT, "--group", "invite"],
      [
        "set",
        "--account",
        ACCOUNT,
        "--group",
        "invite",
        "--wait-s",
        "5",
        "--until",
        "2026-11-07T00:00:00Z",
      ],
      ["set", "--group", "invite", "--wait-s", "5"],
      ["set", "--account", ACCOUNT, "--group", "bogus", "--wait-s", "5"],
      ["set", "--account", ACCOUNT, "--group", "invite", "--wait-s", "0"],
    ]
    for (const args of cases) {
      const result = await runHolds(args)
      expect(result.exitCode).toBe(1)
      expect(result.err.join("\n")).toContain("error:")
    }
    expect(existsSync(stateDbPath(env))).toBe(false)
  })
})

describe("holds clear", () => {
  const args = ["clear", "--account", ACCOUNT, "--group", "invite"]

  test("dry run keeps the hold; --run removes it", async () => {
    seed([
      {
        accountId: ACCOUNT,
        grp: "invite",
        method: "channels.inviteToChannel",
        source: "peer_flood",
        waitS: 86400,
        nowMs: NOW_MS,
      },
    ])
    const dry = await runHolds([...args, "--json"])
    expect(JSON.parse(dry.out[0]!)).toMatchObject({
      mode: "dry_run",
      found: true,
      removed: false,
      source: "peer_flood",
    })
    expect(readHolds()).toHaveLength(1)

    expect(dry.err.join("\n")).toContain("--force")

    // An active hold needs --force.
    const refused = await runHolds([...args, "--run"])
    expect(refused.exitCode).toBe(1)
    expect(refused.err.join("\n")).toContain("add --force")
    expect(readHolds()).toHaveLength(1)

    const run = await runHolds([...args, "--run", "--force"])
    expect(run.out[0]).toContain("removed: true")
    expect(readHolds()).toHaveLength(0)
  })

  test("an ended hold clears with --run alone", async () => {
    seed([
      {
        accountId: ACCOUNT,
        grp: "invite",
        method: "channels.inviteToChannel",
        source: "flood_wait",
        untilMs: NOW_MS - 1_000,
        nowMs: NOW_MS - 60_000,
      },
    ])
    const run = await runHolds([...args, "--run", "--json"])
    expect(run.exitCode).toBeNull()
    expect(JSON.parse(run.out[0]!)).toMatchObject({ removed: true })
  })

  test("missing hold reports found: false", async () => {
    const result = await runHolds([...args, "--run", "--json"])
    expect(JSON.parse(result.out[0]!)).toMatchObject({
      found: false,
      removed: false,
    })
    expect(result.err.join("\n")).toContain("No hold stored")
  })
})

describe("holds write safeguards", () => {
  const setArgs = [
    "set",
    "--account",
    ACCOUNT,
    "--group",
    "invite",
    "--wait-s",
    "600",
    "--run",
  ]
  const clearArgs = ["clear", "--account", ACCOUNT, "--group", "invite"]

  function logRows(): EventRow[] {
    const path = telemetryDbPath(env)
    if (!existsSync(path)) return []
    const db = openTelemetryDb(path)
    try {
      return db
        .query<EventRow, []>(
          "SELECT * FROM event WHERE event LIKE 'holds.%' ORDER BY ts_ms",
        )
        .all()
    } finally {
      db.close()
    }
  }

  test("set and clear write holds.set and holds.clear log rows", async () => {
    expect((await runHolds(setArgs)).exitCode).toBeNull()
    const until = readHolds()[0]!.untilMs
    expect(
      (await runHolds([...clearArgs, "--run", "--force"])).exitCode,
    ).toBeNull()
    const rows = logRows()
    expect(rows.map((row) => [row.event, row.level, row.account_id])).toEqual([
      ["holds.set", "info", Number(ACCOUNT)],
      ["holds.clear", "info", Number(ACCOUNT)],
    ])
    for (const row of rows) {
      expect(JSON.parse(row.attrs!)).toEqual({
        account: Number(ACCOUNT),
        group: { _: "invite" },
        until,
      })
    }
  })

  test("dry runs and refused clears write no log row", async () => {
    seed([
      {
        accountId: ACCOUNT,
        grp: "invite",
        method: "channels.inviteToChannel",
        source: "flood_wait",
        waitS: 600,
        nowMs: NOW_MS,
      },
    ])
    await runHolds(setArgs.filter((arg) => arg !== "--run"))
    await runHolds(clearArgs)
    await runHolds([...clearArgs, "--run"])
    expect(logRows()).toEqual([])
  })

  test("--run refuses a state.db owned by another uid", async () => {
    seed([
      {
        accountId: ACCOUNT,
        grp: "invite",
        method: "channels.inviteToChannel",
        source: "flood_wait",
        waitS: 600,
        nowMs: NOW_MS,
      },
    ])
    const ownerUid = statSync(stateDbPath(env)).uid
    const other = { getuid: () => ownerUid + 1 }
    for (const args of [setArgs, [...clearArgs, "--run", "--force"]]) {
      const result = await runHolds(args, other)
      expect(result.exitCode).toBe(1)
      const text = result.err.join("\n")
      expect(text).toContain(`owned by uid ${ownerUid}`)
      expect(text).toContain(`not uid ${ownerUid + 1}`)
      expect(text).toContain(`sudo -u `)
      expect(text).toContain(`TG_SAI_DIR=${dir}`)
    }
    expect(readHolds()).toHaveLength(1)
    expect(logRows()).toEqual([])
    // Dry runs only read, so they still work.
    const dry = await runHolds(
      setArgs.filter((arg) => arg !== "--run"),
      other,
    )
    expect(dry.exitCode).toBeNull()
  })

  test("foreignOwnerRefusal names the owner and its sudo -u form", () => {
    const path = stateDbPath(env)
    expect(foreignOwnerRefusal(path, 0, "set")).toBeNull() // no file yet
    seed([])
    const uid = statSync(path).uid
    expect(foreignOwnerRefusal(path, uid, "set")).toBeNull()
    expect(foreignOwnerRefusal(path, undefined, "set")).toBeNull()
    expect(
      foreignOwnerRefusal(path, uid + 1, "clear", () => "sai-custody-worker"),
    ).toContain(
      `(sai-custody-worker), not uid ${uid + 1}. Run as the owner, e.g.: ` +
        `sudo -u sai-custody-worker env TG_SAI_DIR=${dir} HOME=${dir} ` +
        "tg-sai holds clear ... --run",
    )
    expect(foreignOwnerRefusal(path, uid + 1, "set", () => null)).toContain(
      `sudo -u '#${uid}' env`,
    )
  })

  test("userNameOf reads /etc/passwd lines", () => {
    const passwd = () =>
      "root:x:0:0:root:/root:/bin/bash\n" +
      "sai-custody-worker:x:998:998::/var/lib/sai-custody-worker:" +
      "/usr/sbin/nologin\n"
    expect(userNameOf(998, passwd)).toBe("sai-custody-worker")
    expect(userNameOf(0, passwd)).toBe("root")
    expect(userNameOf(1234, passwd)).toBeNull()
    expect(
      userNameOf(0, () => {
        throw new Error("no file")
      }),
    ).toBeNull()
  })
})
