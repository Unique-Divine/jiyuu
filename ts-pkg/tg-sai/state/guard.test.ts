import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { RpcCallMiddlewareContext } from "@mtcute/bun"

import {
  HOLD_ERROR_CODE,
  HoldError,
  createFloodGuard,
  holdErrorSeconds,
  type FloodGuard,
} from "./guard.ts"
import {
  HoldStore,
  HoldStoreUnavailableError,
  PEER_FLOOD_WAIT_S,
  holdUntil,
} from "./holds.ts"

const T0 = 1_800_000_000_000

/** Stand-in for the wire: returns queued results and counts calls. */
class FakeWire {
  calls: string[] = []
  constructor(private readonly results: unknown[] = []) {}
  next = async (ctx: RpcCallMiddlewareContext): Promise<unknown> => {
    this.calls.push(String(ctx.request._))
    return this.results.shift() ?? { _: "boolTrue" }
  }
}

function ctxFor(method: string): RpcCallMiddlewareContext {
  return {
    request: { _: method },
    manager: {},
  } as unknown as RpcCallMiddlewareContext
}

/** check -> (basic() would sit here) -> record -> wire. */
function call(
  guard: FloodGuard,
  wire: FakeWire,
  method: string,
): Promise<unknown> {
  return guard.check(ctxFor(method), (c) => guard.record(c, wire.next))
}

function floodWait(seconds: number) {
  return {
    _: "mt_rpc_error",
    errorCode: 420,
    errorMessage: `FLOOD_WAIT_${seconds}`,
  }
}

let dir: string
let store: HoldStore
let nowMs: number

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "tg-sai-guard-"))
  store = HoldStore.open(join(dir, "state.db"))
  nowMs = T0
})

afterEach(() => {
  store.close()
  rmSync(dir, { recursive: true, force: true })
})

function guardFor(
  account: string | number | null | undefined,
  s: HoldStore = store,
): FloodGuard {
  return createFloodGuard(s, () => account, {
    now: () => nowMs,
    requestIdOf: () => "req-1",
  })
}

describe("createFloodGuard", () => {
  test("a recorded FLOOD_WAIT blocks the next call without next", async () => {
    const guard = guardFor(42)
    const wire = new FakeWire([floodWait(300)])
    const first = await call(guard, wire, "channels.inviteToChannel")
    expect(first).toEqual(floodWait(300))
    expect(wire.calls).toHaveLength(1)

    const hold = store.activeHold(42, "invite", nowMs)
    expect(hold?.untilMs).toBe(holdUntil(T0, 300))
    expect(hold?.source).toBe("flood_wait")
    expect(hold?.requestId).toBe("req-1")

    nowMs = T0 + 1000
    // Same group, different method.
    const error = await call(guard, wire, "messages.addChatUser").catch(
      (e: unknown) => e,
    )
    expect(error).toBeInstanceOf(HoldError)
    const held = error as HoldError
    expect(held.code).toBe(HOLD_ERROR_CODE)
    expect(held.message).toBe("TG_SAI_HOLD_309")
    expect(held.text).toBe(held.message)
    expect(held.seconds).toBe(309)
    expect(held.untilMs).toBe(holdUntil(T0, 300))
    expect(held.grp).toBe("invite")
    expect(holdErrorSeconds(held.message)).toBe(309)
    expect(wire.calls).toHaveLength(1)

    // Other groups still go through.
    await call(guard, wire, "contacts.resolveUsername")
    expect(wire.calls).toHaveLength(2)
  })

  test("hold is per account", async () => {
    const wire = new FakeWire([floodWait(60)])
    await call(guardFor(1), wire, "messages.sendMessage")
    await call(guardFor(2), wire, "messages.sendMessage")
    expect(wire.calls).toHaveLength(2)
    await expect(
      call(guardFor(1), wire, "messages.sendMedia"),
    ).rejects.toBeInstanceOf(HoldError)
  })

  test("hold survives reopening the database", async () => {
    const wire = new FakeWire([floodWait(900)])
    await call(guardFor(42), wire, "channels.createChannel")
    store.close()
    store = HoldStore.open(join(dir, "state.db"))
    await expect(
      call(guardFor(42), wire, "messages.createChat"),
    ).rejects.toThrow(/^TG_SAI_HOLD_\d+$/)
    expect(wire.calls).toHaveLength(1)
  })

  test("PEER_FLOOD holds for 24 h", async () => {
    const wire = new FakeWire([
      { _: "mt_rpc_error", errorCode: 400, errorMessage: "PEER_FLOOD" },
    ])
    await call(guardFor(42), wire, "channels.inviteToChannel")
    const hold = store.activeHold(42, "invite", nowMs)
    expect(hold?.source).toBe("peer_flood")
    expect(hold?.waitS).toBe(PEER_FLOOD_WAIT_S)
    expect(hold?.untilMs).toBe(holdUntil(T0, PEER_FLOOD_WAIT_S))
  })

  test("FLOOD_PREMIUM_WAIT is recorded", async () => {
    const wire = new FakeWire([
      {
        _: "mt_rpc_error",
        errorCode: 420,
        errorMessage: "FLOOD_PREMIUM_WAIT_20",
      },
    ])
    await call(guardFor(42), wire, "upload.saveFilePart")
    const hold = store.activeHold(42, "method:upload.saveFilePart", nowMs)
    expect(hold?.source).toBe("flood_premium_wait")
  })

  test("an expired hold passes", async () => {
    const wire = new FakeWire([floodWait(5)])
    await call(guardFor(42), wire, "messages.sendMessage")
    nowMs = holdUntil(T0, 5)
    await call(guardFor(42), wire, "messages.sendMessage")
    expect(wire.calls).toHaveLength(2)
  })

  test("results seen only by check are never recorded", async () => {
    // Hold send; check then throws, so record (innermost) never runs.
    store.recordHold({
      accountId: 42,
      grp: "send",
      method: "messages.sendMessage",
      source: "manual",
      untilMs: T0 + 60_000,
      nowMs: T0,
    })
    const guard = guardFor(42)
    const wire = new FakeWire()
    let recordRan = false
    const synthetic = floodWait(999_999)
    const result = guard.check(ctxFor("messages.sendMessage"), async (c) => {
      recordRan = true
      return guard.record(c, async () => synthetic)
    })
    await expect(result).rejects.toBeInstanceOf(HoldError)
    expect(recordRan).toBe(false)
    expect(wire.calls).toHaveLength(0)
    expect(store.activeHold(42, "send", T0)?.untilMs).toBe(T0 + 60_000)

    // A synthetic stored-wait result returned above record (as basic()'s
    // floodWaiter does) passes through check untouched and unrecorded.
    const outer = await guard.check(ctxFor("contacts.resolveUsername"), () =>
      Promise.resolve(synthetic),
    )
    expect(outer).toBe(synthetic)
    expect(store.activeHold(42, "resolve", T0)).toBeNull()
  })

  test("record returns the result unchanged and never throws", async () => {
    const guard = guardFor(42)
    const result = floodWait(10)
    const wire = new FakeWire([result])
    store.close()
    // Store now unavailable: recording fails silently.
    const out = await guard.record(ctxFor("messages.sendMessage"), wire.next)
    expect(out).toBe(result)
  })

  test("record rethrows errors from next", async () => {
    const guard = guardFor(42)
    await expect(
      guard.record(ctxFor("messages.sendMessage"), async () => {
        throw new Error("network down")
      }),
    ).rejects.toThrow("network down")
  })

  test("unavailable store blocks invite/create, passes the rest", async () => {
    const guard = guardFor(42, HoldStore.unavailable("disk gone"))
    const wire = new FakeWire()
    for (const method of [
      "channels.inviteToChannel",
      "messages.addChatUser",
      "channels.createChannel",
      "messages.createChat",
    ]) {
      await expect(call(guard, wire, method)).rejects.toBeInstanceOf(
        HoldStoreUnavailableError,
      )
    }
    expect(wire.calls).toHaveLength(0)
    await call(guard, wire, "contacts.resolveUsername")
    await call(guard, wire, "messages.sendMessage")
    await call(guard, wire, "users.getFullUser")
    expect(wire.calls).toHaveLength(3)
  })

  test("no account id passes through and records nothing", async () => {
    store.recordHold({
      accountId: 42,
      grp: "invite",
      method: "channels.inviteToChannel",
      source: "manual",
      untilMs: T0 + 60_000,
      nowMs: T0,
    })
    for (const account of [null, undefined, ""]) {
      const wire = new FakeWire([floodWait(100)])
      await call(guardFor(account), wire, "channels.createChannel")
      await call(guardFor(account), wire, "channels.inviteToChannel")
      expect(wire.calls).toHaveLength(2)
    }
    const unavailable = guardFor(null, HoldStore.unavailable("x"))
    const wire = new FakeWire()
    await call(unavailable, wire, "channels.createChannel")
    expect(wire.calls).toHaveLength(1)
    expect(store.activeHolds(undefined, T0).map((h) => h.grp)).toEqual([
      "invite",
    ])
  })

  test("a throwing account provider counts as no account", async () => {
    const guard = createFloodGuard(store, () => {
      throw new Error("no identity yet")
    })
    const wire = new FakeWire()
    await guard.check(ctxFor("channels.createChannel"), wire.next)
    expect(wire.calls).toHaveLength(1)
  })
})

describe("HoldError", () => {
  test("message is TG_SAI_HOLD_<remaining seconds>, at least 1", () => {
    expect(new HoldError(T0 + 1500, "send", T0).message).toBe(
      "TG_SAI_HOLD_2",
    )
    expect(new HoldError(T0, "send", T0).message).toBe("TG_SAI_HOLD_1")
    expect(holdErrorSeconds("TG_SAI_HOLD_STATE_UNAVAILABLE")).toBeNull()
  })
})
