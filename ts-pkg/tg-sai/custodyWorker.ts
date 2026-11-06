import { randomUUID } from "node:crypto"
import { mkdir, open, readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { backgroundEnqueue, openBackgroundQueue } from "./backgroundWorker"
import { configDir } from "./profiles"
import {
  closeProcessHoldStores,
  processHoldStore,
} from "./mtcuteAdapter"
import {
  cloudRetryAt,
  HOLD_STATE_UNAVAILABLE,
  HOLD_STATE_UNAVAILABLE_RETRY_MS,
  provisionSetup,
  safeProvisionError,
} from "./provisioning"
import type {
  BackgroundEnqueue,
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
import type { BackgroundQueue } from "./state/background"
import type { HoldStoreSource } from "./state/guard"
import { HoldStore, remainingSeconds } from "./state/holds"
import type { HoldGroup } from "./state/holds"
import { WORKER_TICK_EVENT } from "./telemetry/db"
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
  /** Once aborted, no new job starts; the running one is cut short. */
  signal?: AbortSignal
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

/** Hold groups a setup job uses: resolve handles, create, add members. */
export const SETUP_HOLD_GROUPS: readonly HoldGroup[] = [
  "create",
  "invite",
  "resolve",
]

/** Groups a setup that already has its chat still uses: no create. */
export const CONFIGURE_HOLD_GROUPS: readonly HoldGroup[] = ["invite", "resolve"]

/** Hold groups that block `setup`: create only matters before its chat. */
export function setupHoldGroups(
  setup: Pick<Setup, "chatId">,
): readonly HoldGroup[] {
  return setup.chatId ? CONFIGURE_HOLD_GROUPS : SETUP_HOLD_GROUPS
}

/**
 * When the custody account's saved flood holds allow setup work again:
 * the latest `untilMs` of its holds on `groups` (default create, invite,
 * and resolve), or `HOLD_STATE_UNAVAILABLE_RETRY_MS` from now when the
 * store cannot be read. `null` when no hold applies. Never throws.
 */
export function setupHoldUntil(
  holds: HoldStoreSource,
  accountId: number,
  nowMs: number,
  groups: readonly HoldGroup[] = SETUP_HOLD_GROUPS,
): { untilMs: number; code: string } | null {
  try {
    const store = typeof holds === "function" ? holds() : holds
    let untilMs = 0
    for (const grp of groups) {
      const hold = store.activeHold(accountId, grp, nowMs)
      if (hold && hold.untilMs > untilMs) untilMs = hold.untilMs
    }
    return untilMs > nowMs
      ? { untilMs, code: `TG_SAI_HOLD_${remainingSeconds(untilMs, nowMs)}` }
      : null
  } catch {
    return {
      untilMs: nowMs + HOLD_STATE_UNAVAILABLE_RETRY_MS,
      code: HOLD_STATE_UNAVAILABLE,
    }
  }
}

/** One bounded page per tick; authoritative queue and local checks both scope jobs. */
export async function runWorkerOnce(
  options: WorkerOptions,
  deps: {
    cloud: ProvisionCloud & PendingQueue
    journal: ProvisionJournal
    /**
     * Saved flood holds; a held account is neither claimed nor contacted.
     * A provider is called each time, so an unavailable store can reopen.
     */
    holds: HoldStoreSource
    telegram: <T>(
      setup: Setup,
      operation: (adapter: TelegramProvisioner) => Promise<T>,
    ) => Promise<T>
    /** Queue for optional members of chats this tick makes ready. */
    background?: BackgroundEnqueue
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
    if (options.signal?.aborted) break
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
    const nowMs = options.now ?? Date.now()
    const hold = setupHoldUntil(
      deps.holds,
      setup.expectedUserId,
      nowMs,
      setupHoldGroups(setup),
    )
    if (hold) {
      results.push(await reportHold(deps.cloud, setup, hold, nowMs))
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
          ...(deps.background ? { background: deps.background } : {}),
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

/**
 * Waiting result for a held account. An incomplete setup this worker already
 * holds the claim for gets `cloudRetryAt` of the hold's end as its cloud
 * `retryAt`, through the
 * same `progress` call a flood retry uses, so Deal Desk shows the time. Other
 * setups are not claimed; Deal Desk cannot store a `retryAt` for them.
 */
async function reportHold(
  cloud: ProvisionCloud,
  setup: Setup,
  hold: { untilMs: number; code: string },
  nowMs: number,
): Promise<WorkerResult> {
  // Same value provisioning saves for a hold, so the cloud time never ends
  // before the local hold and the next tick does not report it again.
  const retryAt =
    cloudRetryAt(hold.untilMs, nowMs) ?? new Date(hold.untilMs).toISOString()
  const result: WorkerResult = {
    requestId: setup.requestId,
    state: "waiting",
    ...(setup.chatId ? { chatId: setup.chatId } : {}),
    retryAt,
    error: hold.code,
  }
  if (setup.state !== "incomplete" || !setup.claimToken) return result
  try {
    await cloud.progress(setup.requestId, setup.claimToken, {
      state: "incomplete",
      ...(setup.chatId ? { chatId: setup.chatId } : {}),
      ...(setup.binding ? { binding: setup.binding } : {}),
      error: hold.code,
      retryAt,
    })
  } catch (error) {
    // The local hold still keeps the job waiting; the cloud may lag.
    return { ...result, error: `${hold.code}:${safeProvisionError(error)}` }
  }
  return result
}

export async function runCustodyWorker(options: {
  custodyProfile: string
  once?: boolean
  run?: boolean
  limit?: number
  intervalMs?: number
  signal?: AbortSignal
  /** Process run: worker.tick rows and per-job telemetry. */
  telemetry?: Pick<TelemetryRun, "sink" | "context" | "client">
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
      withProvisionTelegram(setup, operation, clientTelemetry, options.signal),
    )
  let lastTickRecordedMs: number | null = null
  const recordTick = (tick: {
    startedAt: number
    result?: Awaited<ReturnType<typeof runWorkerOnce>>
    error?: unknown
  }) => {
    if (!telemetry) return
    const nowMs = Date.now()
    if (
      !shouldRecordTick({
        ...tick,
        nowMs,
        lastRecordedMs: lastTickRecordedMs,
      })
    ) {
      return
    }
    lastTickRecordedMs = nowMs
    recordWorkerTick(telemetry, {
      ...tick,
      custodyProfile: options.custodyProfile,
    })
  }
  const execute = async () => {
    const cloud = new CliProvisionCloud()
    // Preview never reaches the hold check, so it opens no state file. A
    // run shares the guard's store, which reopens after a failed open.
    const holds: HoldStoreSource = options.run
      ? () => processHoldStore()
      : HoldStore.unavailable("preview")
    // Setups need the queue for their optional members; without state.db
    // such a setup fails before any Telegram write and retries later.
    let queue: BackgroundQueue | null = null
    if (options.run)
      try {
        queue = openBackgroundQueue()
      } catch {
        queue = null
      }
    let after: string | undefined
    try {
      do {
        const tickStartedAt = Date.now()
        try {
          const result = await runWorkerOnce(
            { ...options, workerId, after },
            {
              cloud,
              journal: new FileProvisionJournal(root),
              holds,
              telegram,
              ...(queue ? { background: backgroundEnqueue(queue) } : {}),
            },
          )
          after = result.nextAfter ?? undefined
          const line = { ...result, observedAt: new Date().toISOString() }
          console.log(JSON.stringify(line))
          recordTick({ startedAt: tickStartedAt, result })
        } catch (error) {
          recordTick({ startedAt: tickStartedAt, error })
          if (options.once) throw error
          const line = {
            state: "worker_tick_failed",
            observedAt: new Date().toISOString(),
            retryInMs: interval,
          }
          console.error(JSON.stringify(line))
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
    } finally {
      queue?.close()
      if (options.run) closeProcessHoldStores()
    }
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

/** Idle ticks still write a `worker.tick` row this often, as a heartbeat. */
export const TICK_HEARTBEAT_MS = 5 * 60 * 1000

/** Job states that mean the tick did no work. */
const IDLE_STATES = new Set(["waiting", "blocked"])

/**
 * Whether a tick earns a `worker.tick` row: it threw, a job did something
 * (any result not `waiting`/`blocked`), it is the first tick, or the last
 * recorded tick is `TICK_HEARTBEAT_MS` old. Idle polls are otherwise skipped
 * so a 5s interval does not write ~17k rows a day.
 */
export function shouldRecordTick(tick: {
  result?: Awaited<ReturnType<typeof runWorkerOnce>>
  error?: unknown
  nowMs: number
  lastRecordedMs: number | null
}): boolean {
  if (tick.error !== undefined) return true
  if (tick.result?.results.some((r) => !IDLE_STATES.has(r.state))) return true
  if (tick.lastRecordedMs === null) return true
  return tick.nowMs - tick.lastRecordedMs >= TICK_HEARTBEAT_MS
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
      event: WORKER_TICK_EVENT,
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
