import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { STATE_MIGRATIONS, openStateDb, stateDbPath } from "./db.ts"
import {
  HoldStore,
  HoldStoreUnavailableError,
  MAX_HOLD_MS,
  PEER_FLOOD_WAIT_S,
  holdUntil,
  methodGroup,
  parseHoldError,
} from "./holds.ts"

let dir: string
let path: string

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "tg-sai-state-"))
  path = join(dir, "nested", "state.db")
})

afterEach(() => {
  rmSync(dir, { recursive: true, force: true })
})

const T0 = 1_800_000_000_000

describe("methodGroup", () => {
  test("maps known methods to groups and others to method:<name>", () => {
    expect(methodGroup("channels.inviteToChannel")).toBe("invite")
    expect(methodGroup("messages.addChatUser")).toBe("invite")
    expect(methodGroup("channels.createChannel")).toBe("create")
    expect(methodGroup("messages.createChat")).toBe("create")
    expect(methodGroup("contacts.resolveUsername")).toBe("resolve")
    for (const m of [
      "messages.sendMessage",
      "messages.sendMedia",
      "messages.sendMultiMedia",
      "messages.forwardMessages",
    ]) {
      expect(methodGroup(m)).toBe("send")
    }
    expect(methodGroup("users.getFullUser")).toBe("method:users.getFullUser")
  })
})

describe("parseHoldError", () => {
  test("parses flood waits and PEER_FLOOD", () => {
    expect(parseHoldError("FLOOD_WAIT_120")).toEqual({
      source: "flood_wait",
      waitS: 120,
    })
    expect(parseHoldError("FLOOD_PREMIUM_WAIT_7")).toEqual({
      source: "flood_premium_wait",
      waitS: 7,
    })
    expect(parseHoldError("PEER_FLOOD")).toEqual({
      source: "peer_flood",
      waitS: PEER_FLOOD_WAIT_S,
    })
  })

  test("ignores other errors", () => {
    for (const m of [
      "SLOWMODE_WAIT_10",
      "FLOOD_WAIT_%d",
      "FLOOD_WAIT_",
      "USER_PRIVACY_RESTRICTED",
      "",
      null,
      undefined,
    ]) {
      expect(parseHoldError(m)).toBeNull()
    }
  })
})

describe("holdUntil", () => {
  test("adds max(10 s, 1 %) margin", () => {
    expect(holdUntil(T0, 60)).toBe(T0 + 60_000 + 10_000)
    expect(holdUntil(T0, 86_400)).toBe(T0 + 86_400_000 + 864_000)
    expect(holdUntil(T0, 0)).toBe(T0 + 10_000)
  })
})

describe("openStateDb", () => {
  test("creates a 0700 dir and 0600 file and applies migrations", () => {
    const db = openStateDb(path)
    const names = db
      .query<{ filename: string }, []>(
        "SELECT filename FROM schema_migrations",
      )
      .all()
      .map((r) => r.filename)
    expect(names).toEqual(STATE_MIGRATIONS.map((m) => m.filename))
    const mode = db
      .query<{ journal_mode: string }, []>("PRAGMA journal_mode")
      .get()
    expect(mode?.journal_mode).toBe("wal")
    const busy = db.query<{ timeout: number }, []>("PRAGMA busy_timeout").get()
    expect(busy?.timeout).toBe(5000)
    db.close()
    expect(statSync(path).mode & 0o777).toBe(0o600)
    expect(statSync(join(dir, "nested")).mode & 0o777).toBe(0o700)
  })

  test("stateDbPath lives in the config dir", () => {
    expect(stateDbPath({ TG_SAI_DIR: dir })).toBe(join(dir, "state.db"))
  })

  test("rejects an invalid source", () => {
    const db = openStateDb(path)
    expect(() =>
      db.run(
        "INSERT INTO flood_hold VALUES ('1', 'send', 1, 'm', 1, 'bad', " +
          "NULL, 1, 1)",
      ),
    ).toThrow()
    db.close()
  })
})

describe("HoldStore", () => {
  test("keeps the later until_ms and its details", () => {
    const store = HoldStore.open(path)
    store.recordHold({
      accountId: 42,
      grp: "invite",
      method: "channels.inviteToChannel",
      source: "flood_wait",
      waitS: 600,
      requestId: "long",
      nowMs: T0,
    })
    const after = store.recordHold({
      accountId: 42,
      grp: "invite",
      method: "messages.addChatUser",
      source: "flood_wait",
      waitS: 30,
      requestId: "short",
      nowMs: T0 + 1000,
    })
    expect(after.untilMs).toBe(holdUntil(T0, 600))
    expect(after.method).toBe("channels.inviteToChannel")
    expect(after.requestId).toBe("long")
    expect(after.createdMs).toBe(T0)
    expect(after.updatedMs).toBe(T0 + 1000)

    const longer = store.recordHold({
      accountId: 42,
      grp: "invite",
      method: "messages.addChatUser",
      source: "peer_flood",
      waitS: PEER_FLOOD_WAIT_S,
      nowMs: T0 + 2000,
    })
    expect(longer.untilMs).toBe(holdUntil(T0 + 2000, PEER_FLOOD_WAIT_S))
    expect(longer.source).toBe("peer_flood")
    expect(longer.method).toBe("messages.addChatUser")
    store.close()
  })

  test("restarts created_ms after an expired hold", () => {
    const store = HoldStore.open(path)
    store.recordHold({
      accountId: "a",
      grp: "send",
      method: "messages.sendMessage",
      source: "flood_wait",
      waitS: 1,
      nowMs: T0,
    })
    const later = T0 + 3_600_000
    const row = store.recordHold({
      accountId: "a",
      grp: "send",
      method: "messages.sendMessage",
      source: "flood_wait",
      waitS: 5,
      nowMs: later,
    })
    expect(row.createdMs).toBe(later)
    expect(row.untilMs).toBe(holdUntil(later, 5))
    store.close()
  })

  test("caps holds at one year", () => {
    const store = HoldStore.open(path)
    const row = store.recordHold({
      accountId: 1,
      grp: "create",
      method: "channels.createChannel",
      source: "flood_wait",
      waitS: 9_999_999_999,
      nowMs: T0,
    })
    expect(row.untilMs).toBe(T0 + MAX_HOLD_MS)
    store.close()
  })

  test("activeHold, activeHolds, and clearHold", () => {
    const store = HoldStore.open(path)
    store.recordHold({
      accountId: 1,
      grp: "invite",
      method: "channels.inviteToChannel",
      source: "flood_wait",
      waitS: 100,
      nowMs: T0,
    })
    store.recordHold({
      accountId: 1,
      grp: "resolve",
      method: "contacts.resolveUsername",
      source: "manual",
      untilMs: T0 + 500_000,
      nowMs: T0,
    })
    store.recordHold({
      accountId: 2,
      grp: "send",
      method: "messages.sendMessage",
      source: "flood_wait",
      waitS: 5,
      nowMs: T0,
    })
    expect(store.activeHold(1, "invite", T0)?.untilMs).toBe(
      holdUntil(T0, 100),
    )
    expect(store.activeHold("1", "invite", holdUntil(T0, 100))).toBeNull()
    expect(store.activeHolds(1, T0).map((h) => h.grp)).toEqual([
      "resolve",
      "invite",
    ])
    expect(store.activeHolds(undefined, T0)).toHaveLength(3)
    expect(store.activeHolds(null, T0 + 200_000)).toHaveLength(1)
    expect(store.clearHold(1, "resolve")).toBe(true)
    expect(store.clearHold(1, "resolve")).toBe(false)
    expect(store.activeHolds(1, T0).map((h) => h.grp)).toEqual(["invite"])
    store.close()
  })

  test("hold survives closing and reopening the database", () => {
    const first = HoldStore.open(path)
    first.recordHold({
      accountId: 7,
      grp: "create",
      method: "channels.createChannel",
      source: "flood_wait",
      waitS: 3600,
      nowMs: T0,
    })
    first.close()
    expect(first.unavailable).toBe(true)
    const second = HoldStore.open(path)
    expect(second.unavailable).toBe(false)
    expect(second.activeHold(7, "create", T0 + 1000)?.waitS).toBe(3600)
    second.close()
  })

  test("a store that fails to open reports unavailable and throws", () => {
    const blocker = join(dir, "file")
    writeFileSync(blocker, "not a dir")
    const store = HoldStore.open(join(blocker, "state.db"))
    expect(store.unavailable).toBe(true)
    expect(store.unavailableReason).toBeTruthy()
    expect(() => store.activeHold(1, "invite")).toThrow(
      HoldStoreUnavailableError,
    )
    expect(() => store.activeHolds()).toThrow("TG_SAI_HOLD_STATE_UNAVAILABLE")
  })

  test("two processes MAX-upsert concurrently", async () => {
    // Prime the schema so both writers race on rows, not on migration.
    HoldStore.open(path).close()
    const holdsUrl = new URL("./holds.ts", import.meta.url).pathname
    const script = join(dir, "writer.ts")
    writeFileSync(
      script,
      `import { HoldStore } from ${JSON.stringify(holdsUrl)}
const [path, offset] = process.argv.slice(2)
const store = HoldStore.open(path)
if (store.unavailable) throw new Error(store.unavailableReason ?? "x")
for (let i = 0; i < 200; i += 1) {
  store.recordHold({
    accountId: 9,
    grp: "invite",
    method: "channels.inviteToChannel",
    source: "flood_wait",
    waitS: (i * 2 + Number(offset)) % 401,
    nowMs: ${T0},
  })
}
store.close()
`,
    )
    const spawn = (offset: number) =>
      Bun.spawn([process.execPath, script, path, String(offset)], {
        stdout: "pipe",
        stderr: "pipe",
      })
    const procs = [spawn(0), spawn(1)]
    const codes = await Promise.all(procs.map((p) => p.exited))
    const errs = await Promise.all(
      procs.map((p) => new Response(p.stderr).text()),
    )
    expect(errs).toEqual(["", ""])
    expect(codes).toEqual([0, 0])
    const store = HoldStore.open(path)
    // Writer 1 reaches wait 399 (i = 199); writer 0 reaches 398.
    const hold = store.activeHold(9, "invite", T0)
    expect(hold?.untilMs).toBe(holdUntil(T0, 399))
    expect(hold?.waitS).toBe(399)
    store.close()
  })
})
