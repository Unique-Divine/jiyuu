import { randomUUID } from "node:crypto"
import { mkdir, open, readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { configDir } from "./profiles"
import { provisionSetup, safeProvisionError } from "./provisioning"
import type {
  ProvisionCloud,
  ProvisionJournal,
  Setup,
  TelegramProvisioner,
} from "./provisioning"
import { CliProvisionCloud } from "./provisioningCloud"
import {
  FileProvisionJournal,
  withAccountLock,
  withProvisionTelegram,
} from "./provisioningRuntime"
import { withTelemetryJob } from "./telemetry/middleware"
import type { TelemetryRun } from "./telemetry/run"

export const CUSTODY_USER_ID = 8274304175
export const REQUIRED_UNIQUE_ADMIN_ID = 1912700300
export interface PendingQueue {
  pending(input: {
    expectedUserId: number
    workerId: string
    limit: number
    after?: string
  }): Promise<{
    requests: Setup[]
    nextAfter: string | null
    blocked?: { requestId: string; state: string; reason: string }[]
  }>
}
export interface WorkerResult {
  requestId: string
  state: string
  chatId?: number
  retryAt?: string | null
  error?: string
}
export interface WorkerOptions {
  custodyProfile: string
  workerId: string
  limit?: number
  run?: boolean
  now?: number
  after?: string
}
/** Persistent random identity retains ownership of sticky cloud claims after restart. */
export async function stableWorkerId(root = configDir()): Promise<string> {
  const path = join(root, "provisioning", "worker-id")
  await mkdir(dirname(path), { recursive: true, mode: 0o700 })
  try {
    const file = await open(path, "wx", 0o600)
    try {
      await file.writeFile(`custody:${CUSTODY_USER_ID}:${randomUUID()}`)
      await file.sync()
    } finally {
      await file.close()
    }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error
  }
  const id = (await readFile(path, "utf8")).trim()
  if (!new RegExp(`^custody:${CUSTODY_USER_ID}:[a-f0-9-]{36}$`).test(id))
    throw new Error(
      "Invalid saved worker identity; inspect private worker state",
    )
  return id
}

/** One bounded page per tick; authoritative queue and local checks both scope jobs. */
export async function runWorkerOnce(
  options: WorkerOptions,
  deps: {
    cloud: ProvisionCloud & PendingQueue
    journal: ProvisionJournal
    telegram: <T>(
      setup: Setup,
      operation: (adapter: TelegramProvisioner) => Promise<T>,
    ) => Promise<T>
  },
): Promise<{
  dryRun: boolean
  results: WorkerResult[]
  nextAfter: string | null
}> {
  if (!options.custodyProfile.trim() || !options.workerId.trim())
    throw new Error("Explicit custody profile and stable worker ID required")
  const limit = options.limit ?? 10
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 50)
    throw new Error("Worker limit must be 1 through 50")
  const page = await deps.cloud.pending({
    expectedUserId: CUSTODY_USER_ID,
    workerId: options.workerId,
    limit,
    ...(options.after ? { after: options.after } : {}),
  })
  if (page.requests.length > limit)
    throw new Error("Pending queue exceeded requested bound")
  if (
    !Array.isArray(page.blocked ?? []) ||
    (page.blocked?.length ?? 0) + page.requests.length > limit
  )
    throw new Error("Pending queue blocked results exceeded requested bound")
  const results: WorkerResult[] = (page.blocked ?? []).map((row) => ({
    requestId: row.requestId,
    state: "blocked",
    error: row.reason,
  }))
  for (const selected of page.requests) {
    const setup = await deps.cloud.view(selected.requestId)
    if (
      setup.expectedUserId !== CUSTODY_USER_ID ||
      setup.custodyProfile !== options.custodyProfile
    )
      throw new Error("Pending job does not match configured custody identity")
    if (
      !setup.admins.some(
        (admin) => String(admin.userId) === String(REQUIRED_UNIQUE_ADMIN_ID),
      )
    ) {
      results.push({
        requestId: setup.requestId,
        state: "blocked",
        error: "required_unique_admin_missing",
      })
      continue
    }
    if (
      !["prepared", "creating", "configuring", "incomplete"].includes(
        setup.state,
      )
    ) {
      results.push({ requestId: setup.requestId, state: setup.state })
      continue
    }
    // A permanent/unknown configuration failure needs explicit operator recovery.
    // Only a persisted transient retry time authorizes unattended retry.
    if (setup.state === "incomplete" && !setup.retryAt) {
      results.push({
        requestId: setup.requestId,
        state: "blocked",
        error: "operator_recovery_required",
      })
      continue
    }
    if (
      setup.retryAt &&
      Date.parse(setup.retryAt) > (options.now ?? Date.now())
    ) {
      results.push({
        requestId: setup.requestId,
        state: "waiting",
        retryAt: setup.retryAt,
      })
      continue
    }
    if (!options.run) {
      results.push({
        requestId: setup.requestId,
        state: setup.state,
        ...(setup.chatId ? { chatId: setup.chatId } : {}),
      })
      continue
    }
    try {
      const result = await deps.telegram(setup, (telegram) =>
        provisionSetup({
          setup,
          telegram,
          cloud: deps.cloud,
          journal: deps.journal,
          workerId: options.workerId,
        }),
      )
      results.push({
        requestId: setup.requestId,
        state: result.state,
        ...("chatId" in result && result.chatId
          ? { chatId: result.chatId }
          : {}),
      })
    } catch (error) {
      // Get the actual state after an ambiguous response; never retry a creation in this tick.
      const current = await deps.cloud.view(setup.requestId)
      results.push({
        requestId: setup.requestId,
        state: current.state,
        ...(current.chatId ? { chatId: current.chatId } : {}),
        retryAt: current.retryAt,
        error: safeProvisionError(error),
      })
    }
  }
  return { dryRun: !options.run, results, nextAfter: page.nextAfter }
}

export async function runCustodyWorker(options: {
  custodyProfile: string
  once?: boolean
  run?: boolean
  limit?: number
  intervalMs?: number
  signal?: AbortSignal
  /** Process run: worker.tick rows, per-job telemetry, and the run log. */
  telemetry?: Pick<TelemetryRun, "sink" | "context" | "client" | "appendJsonl">
}): Promise<void> {
  const interval = options.intervalMs ?? 5000
  if (!Number.isSafeInteger(interval) || interval < 1000 || interval > 60000)
    throw new Error("Worker interval must be 1000 through 60000ms")
  if (!options.once && !options.run)
    throw new Error(
      "Daemon mode requires --run; use --once for read-only preview",
    )
  const root = configDir()
  const workerId = await stableWorkerId(root)
  const telemetry = options.telemetry
  const clientTelemetry = telemetry?.client({
    accountId: CUSTODY_USER_ID,
    profile: options.custodyProfile,
  })
  // Each job's Telegram calls carry its setup ID as job_id.
  const telegram = <T>(
    setup: Setup,
    operation: (adapter: TelegramProvisioner) => Promise<T>,
  ): Promise<T> =>
    withTelemetryJob(setup.requestId, () =>
      withProvisionTelegram(setup, operation, clientTelemetry),
    )
  const execute = async () => {
    const cloud = new CliProvisionCloud()
    let after: string | undefined
    do {
      const tickStartedAt = Date.now()
      try {
        const result = await runWorkerOnce(
          { ...options, workerId, after },
          {
            cloud,
            journal: new FileProvisionJournal(root),
            telegram,
          },
        )
        after = result.nextAfter ?? undefined
        const line = { ...result, observedAt: new Date().toISOString() }
        console.log(JSON.stringify(line))
        if (telemetry) {
          recordWorkerTick(telemetry, {
            startedAt: tickStartedAt,
            custodyProfile: options.custodyProfile,
            result,
          })
          telemetry.appendJsonl({ type: "worker_tick", ...line })
        }
      } catch (error) {
        if (telemetry) {
          recordWorkerTick(telemetry, {
            startedAt: tickStartedAt,
            custodyProfile: options.custodyProfile,
            error,
          })
        }
        if (options.once) throw error
        const line = {
          state: "worker_tick_failed",
          observedAt: new Date().toISOString(),
          retryInMs: interval,
        }
        console.error(JSON.stringify(line))
        telemetry?.appendJsonl({ type: "worker_tick_failed", ...line })
      }
      if (options.once || options.signal?.aborted) break
      await new Promise<void>((resolve) => {
        const done = () => {
          clearTimeout(timer)
          options.signal?.removeEventListener("abort", done)
          resolve()
        }
        const timer = setTimeout(done, interval)
        options.signal?.addEventListener("abort", done, { once: true })
      })
    } while (!options.signal?.aborted)
  }
  if (!options.run) return execute()
  await withAccountLock(
    join(root, "provisioning", "worker-service.lock"),
    execute,
  )
}

/** Counts results per state, e.g. `{ ready: 2, waiting: 1 }`. */
export function countStates(results: WorkerResult[]): Record<string, number> {
  const counts: Record<string, number> = {}
  for (const result of results) {
    counts[result.state] = (counts[result.state] ?? 0) + 1
  }
  return counts
}

/** Writes one `worker.tick` row; never throws. */
export function recordWorkerTick(
  telemetry: Pick<TelemetryRun, "sink" | "context">,
  tick: {
    startedAt: number
    custodyProfile: string
    result?: Awaited<ReturnType<typeof runWorkerOnce>>
    error?: unknown
  },
): void {
  try {
    const { context } = telemetry
    telemetry.sink.record({
      runId: context.runId,
      subsystem: context.subsystem,
      command: context.command,
      version: context.version,
      accountId: CUSTODY_USER_ID,
      profile: tick.custodyProfile,
      event: "worker.tick",
      outcome: tick.error === undefined ? "ok" : "error",
      durationMs: Date.now() - tick.startedAt,
      err:
        tick.error === undefined ? null : safeProvisionError(tick.error),
      res: tick.result
        ? {
            dryRun: tick.result.dryRun,
            jobs: tick.result.results.length,
            states: countStates(tick.result.results),
            more: tick.result.nextAfter !== null,
          }
        : null,
    })
  } catch {
    // Telemetry never stops the worker.
  }
}
