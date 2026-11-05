import { expect, test } from "bun:test"
import { mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Command } from "commander"
import { CUSTODY_USER_ID, runWorkerOnce, stableWorkerId } from "./custodyWorker"
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
    progress: async (_id: string, _token: string, progress: Progress) => {
      setup = { ...setup, ...progress }
      return structuredClone(setup)
    },
    resolve: async () => setup,
  }
  return {
    calls,
    cloud,
    journal: { read: async () => null, write: async () => {} },
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
