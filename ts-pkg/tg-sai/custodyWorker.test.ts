import { expect, setSystemTime, test } from "bun:test"
import { mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { MtcuteError, MtTimeoutError } from "@mtcute/bun"
import { Command } from "commander"
import {
  CUSTODY_USER_ID,
  recordWorkerTick,
  runWorkerOnce,
  setupHoldGroups,
  setupHoldUntil,
  shouldRecordTick,
  stableWorkerId,
  TICK_HEARTBEAT_MS,
} from "./custodyWorker"
import { openStateDb } from "./state/db"
import { HoldStore } from "./state/holds"
import type { HoldGroup } from "./state/holds"
import type { TelemetryEvent } from "./telemetry/db"
import { addCustodyWorkerCommands } from "./custodyWorkerCli"
import { MIN_CLOUD_RETRY_MS, PROVISIONING_BOT_ID } from "./provisioning"
import type { JournalRecord, Member } from "./provisioning"
import type { Progress, Setup, TelegramProvisioner } from "./provisioning"
import { CliProvisionCloud } from "./provisioningCloud"

function fixture() {
  let setup: Setup = {
    requestId: "worker-test",
    custodyProfile: "custody",
    expectedUserId: CUSTODY_USER_ID,
    title: "Sai test",
    participants: [1912700300],
    admins: [{ userId: 1912700300, rightsProfile: "team" }],
    botRightsProfile: "baseline",
    dealIds: [],
    state: "prepared",
  }
  const calls: string[] = []
  const progressed: Progress[] = []
  const cloud = {
    pending: async (input: {
      expectedUserId: number
      workerId: string
      limit: number
      after?: string
    }) => {
      calls.push(
        `pending:${input.expectedUserId}:${input.workerId}:${input.limit}:${input.after ?? ""}`,
      )
      return { requests: [structuredClone(setup)], nextAfter: null }
    },
    view: async () => structuredClone(setup),
    claim: async () => {
      calls.push("claim")
      return { ...setup, claimToken: "private-token" }
    },
    progress: async (_id: string, token: string, progress: Progress) => {
      calls.push(`progress:${token}`)
      progressed.push(progress)
      // Like Deal Desk, only awaiting_join keeps a next check time.
      setup = {
        ...setup,
        ...progress,
        nextCheckAt: progress.nextCheckAt ?? null,
      }
      return structuredClone(setup)
    },
    resolve: async () => setup,
  }
  return {
    calls,
    progressed,
    cloud,
    journal: { read: async () => null, write: async () => {} },
    holds: HoldStore.fromDb(openStateDb(":memory:")),
    telegram: async <T>(
      _setup: Setup,
      operation: (adapter: TelegramProvisioner) => Promise<T>,
    ) => {
      calls.push("telegram")
      return await operation({
        self: async () => CUSTODY_USER_ID,
      } as TelegramProvisioner)
    },
    get setup() {
      return setup
    },
    set setup(value: Setup) {
      setup = value
    },
  }
}
test("one-page preview is bounded and invokes no Telegram or claim", async () => {
  const f = fixture()
  expect(
    await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", limit: 3, after: "old" },
      f,
    ),
  ).toEqual({
    dryRun: true,
    results: [{ requestId: "worker-test", state: "prepared" }],
    nextAfter: null,
    requesterWork: true,
  })
  expect(f.calls).toEqual([`pending:${CUSTODY_USER_ID}:stable:3:old`])
})
test("worker rejects account/profile mismatches before connecting", async () => {
  for (const patch of [{ expectedUserId: 1 }, { custodyProfile: "other" }]) {
    const f = fixture()
    f.setup = { ...f.setup, ...patch }
    await expect(
      runWorkerOnce(
        { custodyProfile: "custody", workerId: "stable", run: true },
        f,
      ),
    ).rejects.toThrow("custody identity")
    expect(f.calls).not.toContain("telegram")
  }
})
test("worker leaves unknown creation for explicit recovery, waits for future retry, and blocks missing Unique", async () => {
  for (const [patch, state] of [
    [{ state: "creation_unknown" }, "creation_unknown"],
    [{ state: "incomplete" }, "blocked"],
    [{ retryAt: "2100-01-01T00:00:00Z" }, "waiting"],
    [{ admins: [] }, "blocked"],
  ] as const) {
    const f = fixture()
    f.setup = { ...f.setup, ...patch } as Setup
    const result = await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", run: true },
      f,
    )
    expect(result.results[0]?.state).toBe(state)
    expect(f.calls).not.toContain("telegram")
  }
})
test("worker identity is persistent and private state survives restart", async () => {
  const dir = await mkdtemp(join(tmpdir(), "custody-worker-id-"))
  try {
    const one = await stableWorkerId(dir)
    expect(one).toMatch(new RegExp(`^custody:${CUSTODY_USER_ID}:`))
    expect(await stableWorkerId(dir)).toBe(one)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})
test("pending queue cloud adapter preserves bounded worker identity and decodes optional bindings", async () => {
  const f = fixture()
  const calls: unknown[] = []
  const cloud = new CliProvisionCloud(async (operation, input) => {
    calls.push({ operation, input })
    return {
      requests: [
        {
          ...f.setup,
          expectedUserId: String(CUSTODY_USER_ID),
          resolvedParticipants: ["1912700300"],
          resolvedAdmins: ["1912700300"],
          resolvedOptionalParticipants: [],
        },
      ],
      nextAfter: "next",
    }
  })
  const result = await cloud.pending({
    expectedUserId: CUSTODY_USER_ID,
    workerId: "stable",
    limit: 2,
  })
  expect(calls).toEqual([
    {
      operation: "pending",
      input: {
        expectedUserId: String(CUSTODY_USER_ID),
        workerId: "stable",
        limit: 2,
      },
    },
  ])
  expect(result.requests[0]?.binding?.optionalParticipants).toEqual([])
  expect(result.nextAfter).toBe("next")
})

test("interrupted creating job is marked unknown and never retried as a new group", async () => {
  const f = fixture()
  f.setup = { ...f.setup, state: "creating" }
  f.telegram = async <T>(
    _setup: Setup,
    operation: (adapter: TelegramProvisioner) => Promise<T>,
  ) => {
    f.calls.push("telegram")
    return operation({
      self: async () => CUSTODY_USER_ID,
      candidates: async () => [-100123],
    } as unknown as TelegramProvisioner)
  }
  const result = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true },
    f,
  )
  expect(result.results[0]?.state).toBe("creation_unknown")
  expect(f.setup.state).toBe("creation_unknown")
  expect(f.calls.filter((call) => call === "claim")).toHaveLength(1)
})

test("a transient Telegram error waits for its retryAt instead of blocking", async () => {
  for (const error of [
    new MtTimeoutError(10_000),
    // Worker shutdown destroys the client mid-call.
    new MtcuteError("Connection destroyed"),
  ]) {
    const f = fixture()
    f.setup = {
      ...f.setup,
      state: "configuring",
      chatId: -1001234567890,
      binding: { participants: [1912700300], admins: [1912700300] },
    }
    f.telegram = async <T>(
      _setup: Setup,
      operation: (adapter: TelegramProvisioner) => Promise<T>,
    ) => {
      f.calls.push("telegram")
      return operation({
        self: async () => CUSTODY_USER_ID,
        verifyMarker: async () => {
          throw error
        },
      } as unknown as TelegramProvisioner)
    }
    const now = Date.now()
    const first = await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", run: true, now },
      f,
    )
    expect(first.results[0]?.state).toBe("incomplete")
    expect(first.results[0]?.retryAt).toBeTruthy()
    expect((f.setup as Setup & { error?: string }).error).toBe(
      "verify_marker:TG_SAI_TRANSIENT",
    )
    f.calls.length = 0
    const next = await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", run: true, now },
      f,
    )
    expect(next.results[0]).toMatchObject({
      state: "waiting",
      retryAt: f.setup.retryAt,
    })
    expect(f.calls).not.toContain("telegram")
  }
})
test("worker command defaults to one-page preview and requires explicit --run for writes", async () => {
  for (const write of [false, true]) {
    const program = new Command()
    let input:
      | Parameters<typeof import("./custodyWorker").runCustodyWorker>[0]
      | undefined
    addCustodyWorkerCommands(program, async (options) => {
      input = options
    })
    await program.parseAsync(
      ["worker", "--custody-profile", "custody", ...(write ? ["--run"] : [])],
      { from: "user" },
    )
    expect(input?.custodyProfile).toBe("custody")
    expect(Boolean(input?.run)).toBe(write)
    expect(Boolean(input?.once)).toBe(!write)
    expect(input?.limit).toBe(10)
    expect(input?.intervalMs).toBe(5000)
  }
})

test("worker surfaces revoked or missing-config job blocks without claims", async () => {
  const f = fixture()
  f.cloud.pending = async () => ({
    requests: [],
    nextAfter: null,
    blocked: [
      { requestId: "revoked", state: "prepared", reason: "operator_revoked" },
    ],
  })
  const result = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true },
    f,
  )
  expect(result.results).toEqual([
    { requestId: "revoked", state: "blocked", error: "operator_revoked" },
  ])
  expect(f.calls).toEqual([])
})

test("worker.tick rows summarize the page and never throw", () => {
  const events: TelemetryEvent[] = []
  const telemetry = {
    context: {
      runId: "run-w",
      subsystem: "worker" as const,
      command: "worker",
      version: "0.0.0-test",
    },
    sink: {
      record: (event: TelemetryEvent) => {
        events.push(event)
      },
      dropped: () => 0,
      close: () => {},
    },
  }
  recordWorkerTick(telemetry, {
    startedAt: Date.now(),
    custodyProfile: "custody",
    result: {
      dryRun: false,
      nextAfter: null,
      requesterWork: true,
      results: [
        { requestId: "a", state: "ready" },
        { requestId: "b", state: "ready" },
        { requestId: "c", state: "waiting" },
      ],
    },
  })
  recordWorkerTick(telemetry, {
    startedAt: Date.now(),
    custodyProfile: "custody",
    error: { text: "FLOOD_WAIT_120" },
  })
  expect(events).toMatchObject([
    {
      event: "worker.tick",
      runId: "run-w",
      accountId: CUSTODY_USER_ID,
      outcome: "ok",
      res: { jobs: 3, states: { ready: 2, waiting: 1 }, more: false },
    },
    { event: "worker.tick", outcome: "error", err: "FLOOD_WAIT_120" },
  ])
  const throwing = {
    ...telemetry,
    sink: { ...telemetry.sink, record: () => { throw new Error("x") } },
  }
  expect(() =>
    recordWorkerTick(throwing, {
      startedAt: 0,
      custodyProfile: "custody",
    }),
  ).not.toThrow()
})

test("an aborted signal starts no new jobs", async () => {
  const f = fixture()
  const controller = new AbortController()
  controller.abort()
  expect(
    await runWorkerOnce(
      {
        custodyProfile: "custody",
        workerId: "stable",
        run: true,
        signal: controller.signal,
      },
      f,
    ),
  ).toEqual({
    dryRun: false,
    results: [],
    nextAfter: null,
    requesterWork: false,
  })
  expect(f.calls).toEqual([`pending:${CUSTODY_USER_ID}:stable:10:`])
})

test("shouldRecordTick skips idle polls between heartbeats", () => {
  const tick = (states: string[]) => ({
    dryRun: false,
    nextAfter: null,
    requesterWork: false,
    results: states.map((state, i) => ({ requestId: `r${i}`, state })),
  })
  const t0 = 1_000_000
  const cases: [Parameters<typeof shouldRecordTick>[0], boolean][] = [
    [{ result: tick([]), nowMs: t0, lastRecordedMs: null }, true],
    [{ result: tick([]), nowMs: t0 + 5000, lastRecordedMs: t0 }, false],
    [
      {
        result: tick(["waiting", "blocked"]),
        nowMs: t0 + 5000,
        lastRecordedMs: t0,
      },
      false,
    ],
    [
      {
        result: tick(["waiting", "ready"]),
        nowMs: t0 + 5000,
        lastRecordedMs: t0,
      },
      true,
    ],
    [{ error: new Error("x"), nowMs: t0 + 1, lastRecordedMs: t0 }, true],
    [
      {
        result: tick(["waiting"]),
        nowMs: t0 + TICK_HEARTBEAT_MS,
        lastRecordedMs: t0,
      },
      true,
    ],
  ]
  for (const [input, expected] of cases) {
    expect(shouldRecordTick(input)).toBe(expected)
  }
})

function hold(
  holds: HoldStore,
  grp: HoldGroup,
  untilMs: number,
  accountId: number = CUSTODY_USER_ID,
) {
  holds.recordHold({
    accountId,
    grp,
    method: "test.method",
    source: "flood_wait",
    untilMs,
    // recordHold caps holds at one year after nowMs.
    nowMs: untilMs - 1,
  })
}

test("an active create, invite, or resolve hold blocks claim and Telegram", async () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  for (const grp of ["create", "invite", "resolve"] as const) {
    const f = fixture()
    hold(f.holds, grp, now + 60_000)
    // The latest hold across the setup groups decides the retry time.
    hold(f.holds, "invite", now + 3_600_000)
    const result = await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", run: true, now },
      f,
    )
    expect(result.results).toEqual([
      {
        requestId: "worker-test",
        state: "waiting",
        retryAt: new Date(now + 3_600_000).toISOString(),
        error: "TG_SAI_HOLD_3600",
      },
    ])
    expect(f.calls).not.toContain("claim")
    expect(f.calls).not.toContain("telegram")
    // An unclaimed prepared setup has no claim token to report with.
    expect(f.progressed).toEqual([])
  }
})

test("holds on other groups or accounts do not block setup work", async () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  const f = fixture()
  hold(f.holds, "send", now + 60_000)
  hold(f.holds, "create", now + 60_000, 42)
  hold(f.holds, "invite", now - 1)
  expect(setupHoldUntil(f.holds, CUSTODY_USER_ID, now)).toBeNull()
  await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now },
    f,
  )
  expect(f.calls).toContain("telegram")
})

test("a held incomplete setup reports the hold as its cloud retryAt and resumes after it", async () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  const untilMs = now + 600_000
  const f = fixture()
  const binding = { participants: [1912700300], admins: [1912700300] }
  f.setup = {
    ...f.setup,
    state: "incomplete",
    chatId: -100123,
    binding,
    claimToken: "private-token",
    retryAt: new Date(now - 1000).toISOString(),
  }
  hold(f.holds, "invite", untilMs)
  const held = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now },
    f,
  )
  const retryAt = new Date(untilMs).toISOString()
  expect(held.results).toEqual([
    {
      requestId: "worker-test",
      state: "waiting",
      chatId: -100123,
      retryAt,
      error: "TG_SAI_HOLD_600",
    },
  ])
  expect(f.progressed).toEqual([
    {
      state: "incomplete",
      chatId: -100123,
      binding,
      error: "TG_SAI_HOLD_600",
      retryAt,
    },
  ])
  expect(f.calls).not.toContain("claim")
  expect(f.calls).not.toContain("telegram")
  expect(f.setup.retryAt).toBe(retryAt)
  // Before the hold ends the saved retryAt alone keeps the job waiting.
  const early = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now: now + 1 },
    f,
  )
  expect(early.results[0]?.state).toBe("waiting")
  expect(f.progressed).toHaveLength(1)
  expect(f.calls).not.toContain("telegram")
  await runWorkerOnce(
    {
      custodyProfile: "custody",
      workerId: "stable",
      run: true,
      now: untilMs + 1,
    },
    f,
  )
  expect(f.calls).toContain("telegram")
})

test("an unavailable hold store waits five minutes without claim or Telegram", async () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  const f = fixture()
  f.holds = HoldStore.unavailable("disk full")
  const result = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now },
    f,
  )
  expect(result.results).toEqual([
    {
      requestId: "worker-test",
      state: "waiting",
      retryAt: new Date(now + 5 * 60_000).toISOString(),
      error: "TG_SAI_HOLD_STATE_UNAVAILABLE",
    },
  ])
  expect(f.calls).not.toContain("claim")
  expect(f.calls).not.toContain("telegram")
})

test("a setup that already has its chat ignores create holds", async () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  expect(setupHoldGroups({})).toEqual(["create", "invite", "resolve"])
  expect(setupHoldGroups({ chatId: -100123 })).toEqual(["invite", "resolve"])

  const configuring = () => {
    const f = fixture()
    f.setup = {
      ...f.setup,
      state: "configuring",
      chatId: -100123,
      binding: { participants: [1912700300], admins: [1912700300] },
    }
    return f
  }
  const created = configuring()
  hold(created.holds, "create", now + 3_600_000)
  await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now },
    created,
  )
  expect(created.calls).toContain("telegram")

  for (const grp of ["invite", "resolve"] as const) {
    const f = configuring()
    hold(f.holds, "create", now + 7_200_000)
    hold(f.holds, grp, now + 60_000)
    const result = await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", run: true, now },
      f,
    )
    // Only the invite/resolve hold sets the time, not the longer create one.
    expect(result.results).toEqual([
      {
        requestId: "worker-test",
        state: "waiting",
        chatId: -100123,
        retryAt: new Date(now + 60_000).toISOString(),
        error: "TG_SAI_HOLD_60",
      },
    ])
    expect(f.calls).not.toContain("telegram")
  }
})

test("a held setup whose retry released its claim is claimed before the hold is saved", async () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  const untilMs = now + 600_000
  const f = fixture()
  f.setup = {
    ...f.setup,
    state: "incomplete",
    chatId: -100123,
    binding: { participants: [1912700300], admins: [1912700300] },
    retryAt: new Date(now - 1000).toISOString(),
  }
  delete f.setup.claimToken
  hold(f.holds, "invite", untilMs)
  const held = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now },
    f,
  )
  expect(held.results[0]).toMatchObject({
    state: "waiting",
    retryAt: new Date(untilMs).toISOString(),
    error: "TG_SAI_HOLD_600",
  })
  expect(f.calls.filter((c) => c === "claim" || c.startsWith("progress:")))
    .toEqual(["claim", "progress:private-token"])
  expect(f.calls).not.toContain("telegram")
})

test("a hold about to end reports retryAt at least 30 s out, once", async () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  const f = fixture()
  f.setup = {
    ...f.setup,
    state: "incomplete",
    chatId: -100123,
    binding: { participants: [1912700300], admins: [1912700300] },
    claimToken: "private-token",
    retryAt: new Date(now - 1000).toISOString(),
  }
  hold(f.holds, "invite", now + 5_000)
  const held = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now },
    f,
  )
  const retryAt = new Date(now + MIN_CLOUD_RETRY_MS).toISOString()
  expect(held.results[0]).toMatchObject({ state: "waiting", retryAt })
  expect(f.progressed.map((p) => p.retryAt)).toEqual([retryAt])
  // The hold has ended but the cloud time has not: no second report.
  const later = await runWorkerOnce(
    {
      custodyProfile: "custody",
      workerId: "stable",
      run: true,
      now: now + 10_000,
    },
    f,
  )
  expect(later.results[0]?.state).toBe("waiting")
  expect(f.progressed).toHaveLength(1)
  expect(f.calls).not.toContain("telegram")
})

test("a hold store provider is asked on each tick, so it can recover", async () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  const f = fixture()
  const open = f.holds
  let current = HoldStore.unavailable("disk I/O error")
  const deps = { ...f, holds: () => current }
  const first = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now },
    deps,
  )
  expect(first.results[0]).toMatchObject({
    state: "waiting",
    error: "TG_SAI_HOLD_STATE_UNAVAILABLE",
  })
  expect(f.calls).not.toContain("telegram")
  current = open
  await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true, now },
    deps,
  )
  expect(f.calls).toContain("telegram")
})
test("a runnable or waiting requester job goes ahead of the background drain", async () => {
  const f = fixture()
  let drains = 0
  const drain = async () => {
    drains++
    return { state: "added" as const, chatId: -1, userId: 2 }
  }
  // The fixture's setup job is runnable: no drain this tick.
  const busy = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true },
    { ...f, drain },
  )
  expect(f.calls).toContain("telegram")
  expect(busy.background).toBeUndefined()
  expect(drains).toBe(0)
  // An empty queue page drains one background add.
  const idle = fixture()
  idle.cloud.pending = async () => ({ requests: [], nextAfter: null })
  const result = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true },
    { ...idle, drain },
  )
  expect(result.background).toEqual({ state: "added", chatId: -1, userId: 2 })
  expect(drains).toBe(1)
  // More pages waiting or a preview: still no drain.
  const more = fixture()
  more.cloud.pending = async () =>
    ({ requests: [], nextAfter: "next" }) as never
  await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true },
    { ...more, drain },
  )
  await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable" },
    { ...idle, drain },
  )
  expect(drains).toBe(1)
  // A job waiting on its retryAt or on any saved hold group blocks the
  // drain too: a flood from the add would hold the requester.
  const waiting = fixture()
  waiting.setup = { ...waiting.setup, retryAt: "2100-01-01T00:00:00Z" }
  const retrying = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true },
    { ...waiting, drain },
  )
  expect(retrying.results[0]?.state).toBe("waiting")
  expect(retrying.background).toBeUndefined()
  const now = Date.parse("2026-11-06T00:00:00Z")
  for (const grp of ["create", "resolve", "invite"] as const) {
    const heldOff = fixture()
    hold(heldOff.holds, grp, now + 600_000)
    const result = await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", run: true, now },
      { ...heldOff, drain },
    )
    expect(result.results[0]?.state).toBe("waiting")
    expect(result.background).toBeUndefined()
    expect(heldOff.calls).not.toContain("telegram")
  }
  expect(drains).toBe(1)
  // Blocked or terminal jobs do not hold up the drain.
  const blocked = fixture()
  blocked.setup = { ...blocked.setup, state: "incomplete" }
  const ready = fixture()
  ready.setup = { ...ready.setup, state: "ready" }
  for (const f2 of [blocked, ready]) {
    const result = await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", run: true },
      { ...f2, drain },
    )
    expect(result.background?.state).toBe("added")
  }
  expect(drains).toBe(3)
  // A waiting job on an earlier page of the pass blocks the last page too.
  const lastPage = fixture()
  lastPage.cloud.pending = async () => ({ requests: [], nextAfter: null })
  const carried = await runWorkerOnce(
    {
      custodyProfile: "custody",
      workerId: "stable",
      run: true,
      after: "page-2",
      requesterWorkSeen: true,
    },
    { ...lastPage, drain },
  )
  expect(carried.background).toBeUndefined()
  expect(carried.requesterWork).toBe(true)
  expect(drains).toBe(3)
  expect(retrying.requesterWork).toBe(true)
  expect(result.requesterWork).toBe(false)
})
test("a failing drain is reported in the tick, never thrown", async () => {
  const f = fixture()
  f.cloud.pending = async () => ({ requests: [], nextAfter: null })
  const result = await runWorkerOnce(
    { custodyProfile: "custody", workerId: "stable", run: true },
    {
      ...f,
      drain: async () => {
        throw new Error("database is locked: /private/path")
      },
    },
  )
  expect(result.background).toEqual({
    state: "error",
    error: "provisioning_step_failed",
  })
  expect(
    shouldRecordTick({ result, nowMs: 1, lastRecordedMs: 0 }),
  ).toBe(true)
  expect(
    shouldRecordTick({
      result: { ...result, background: { state: "idle" } },
      nowMs: 1,
      lastRecordedMs: 0,
    }),
  ).toBe(false)
})

test("a member added but never visible waits without stalling the drain, then blocks", async () => {
  const start = Date.parse("2026-11-10T00:00:00Z")
  setSystemTime(start)
  try {
    const f = fixture()
    const chatId = -100555
    f.setup = {
      ...f.setup,
      state: "configuring",
      chatId,
      binding: { participants: [1912700300], admins: [1912700300] },
    }
    let local: JournalRecord | null = null
    const journal = {
      read: async () => structuredClone(local),
      write: async (_id: string, record: JournalRecord) => {
        local = structuredClone(record)
      },
    }
    const members = new Map<number, Member>([
      [CUSTODY_USER_ID, { role: "creator", rights: {} }],
      [PROVISIONING_BOT_ID, { role: "member", rights: {} }],
    ])
    const adds: number[] = []
    const adapter = {
      self: async () => CUSTODY_USER_ID,
      verifyMarker: async () => true,
      member: async (_chat: number, id: number) =>
        members.get(id) ?? { role: "absent", rights: {} },
      // Telegram accepts the add, but the member never shows up.
      add: async (_chat: number, id: number) => {
        adds.push(id)
      },
      inviteLink: async () => ({
        inviteLink: "https://t.me/+notVisible",
        expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      }),
      approveJoin: async () => false,
      promote: async () => {},
      resolve: async () => ({ id: 0, isBot: false }),
      create: async () => chatId,
      candidates: async () => [],
    } as unknown as TelegramProvisioner
    const telegram = async <T>(
      _setup: Setup,
      operation: (adapter: TelegramProvisioner) => Promise<T>,
    ) => operation(adapter)
    let drains = 0
    const drain = async () => {
      drains++
      return { state: "idle" as const }
    }
    const deps = { ...f, journal, telegram, drain }
    const tick = () =>
      runWorkerOnce(
        { custodyProfile: "custody", workerId: "stable", run: true },
        deps,
      )

    // Tick 1 runs the job: one add, then a link and a 15-minute check.
    // The setup waits as awaiting_join, which holds no Deal Desk claim.
    const first = await tick()
    expect(first.results[0]?.state).toBe("awaiting_join")
    expect(first.results[0]?.retryAt).toBe(f.setup.nextCheckAt)
    expect(f.setup.nextCheckAt).toBeTruthy()
    expect(f.setup.retryAt).toBeFalsy()
    expect(f.setup.error).toBe("verify_added_member:TG_SAI_MEMBER_NOT_VISIBLE")
    expect(adds).toEqual([1912700300])
    expect(drains).toBe(0)

    // While it waits on the invited member, background adds still run.
    setSystemTime(start + 60_000)
    const waiting = await tick()
    expect(waiting.results[0]?.state).toBe("waiting")
    expect(waiting.requesterWork).toBe(false)
    expect(drains).toBe(1)

    // Resumes before expiry never add again, and keep waiting.
    setSystemTime(start + 16 * 60_000)
    await tick()
    expect(adds).toEqual([1912700300])
    expect(f.setup.state).toBe("awaiting_join")

    // After the invitation expires the job stops for an operator.
    setSystemTime(start + 25 * 3_600_000)
    const expired = await tick()
    expect(expired.results[0]?.state).toBe("incomplete")
    expect(f.setup.error).toBe("verify_added_member:TG_SAI_INVITATION_EXPIRED")
    expect(f.setup.retryAt).toBeNull()
    const blocked = await tick()
    expect(blocked.results[0]).toMatchObject({
      state: "blocked",
      error: "operator_recovery_required",
    })
    expect(blocked.background).toBeDefined()
    expect(adds).toEqual([1912700300])
  } finally {
    setSystemTime()
  }
})

test("other waiting jobs still hold up the drain", async () => {
  const f = fixture()
  let drains = 0
  const drain = async () => {
    drains++
    return { state: "idle" as const }
  }
  for (const error of [
    "add_member:FLOOD_WAIT_600",
    "verify_added_member:provisioning_step_failed",
    null,
  ]) {
    f.setup = {
      ...f.setup,
      state: "incomplete",
      retryAt: "2100-01-01T00:00:00Z",
      error,
    }
    const result = await runWorkerOnce(
      { custodyProfile: "custody", workerId: "stable", run: true },
      { ...f, drain },
    )
    expect(result.results[0]?.state).toBe("waiting")
    expect(result.requesterWork).toBe(true)
  }
  expect(drains).toBe(0)
})
