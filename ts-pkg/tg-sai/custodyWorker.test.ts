import { expect, test } from "bun:test"
import { mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Command } from "commander"
import {
  CUSTODY_USER_ID,
  recordWorkerTick,
  runWorkerOnce,
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
      setup = { ...setup, ...progress }
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
  ).toEqual({ dryRun: false, results: [], nextAfter: null })
  expect(f.calls).toEqual([`pending:${CUSTODY_USER_ID}:stable:10:`])
})

test("shouldRecordTick skips idle polls between heartbeats", () => {
  const tick = (states: string[]) => ({
    dryRun: false,
    nextAfter: null,
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
