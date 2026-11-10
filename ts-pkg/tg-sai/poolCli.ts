import { join } from "node:path"
import type { Command } from "commander"
import { CUSTODY_USER_ID } from "./custodyWorker"
import { failureOf } from "./poolFiller"
import type { PoolCloud } from "./poolFiller"
import { configDir } from "./profiles"
import { CliProvisionCloud } from "./provisioningCloud"
import { withAccountLock } from "./provisioningRuntime"
import { openStateDb, stateDbPath } from "./state/db"
import { BackgroundQueue } from "./state/background"
import { PoolJournal } from "./state/pool"
import type { PoolFill } from "./state/pool"

/**
 * Retires the open pool fill `ref`, so the filler starts new fills again,
 * and tells Deal Desk with `pool-fail`, which retires the chat there. A
 * `failed` fill is reported again with its saved stage and error, since
 * the first report was best effort (Deal Desk dedupes the same input). An
 * `active` fill with a chat is reported with stage `other` and error
 * `operator_retired`. The Telegram chat is left as it is. Without `run` it
 * only returns the record.
 */
export async function retirePoolFill(params: {
  journal: PoolJournal
  cloud: Pick<PoolCloud, "poolFail">
  ref: string
  run: boolean
  nowMs?: number
}): Promise<{ dryRun: boolean; fill: PoolFill; reported: boolean }> {
  const fill = params.journal.get(params.ref)
  if (!fill) throw new Error("No pool fill with that ref")
  if (fill.status !== "active" && fill.status !== "failed")
    throw new Error(`Pool fill is already ${fill.status}`)
  if (!params.run) return { dryRun: true, fill, reported: false }
  let reported = false
  if (fill.status === "failed" || fill.chatId !== null) {
    const failure =
      fill.status === "failed"
        ? failureOf(fill.lastError)
        : { stage: "other" as const, error: "operator_retired" }
    await params.cloud.poolFail({
      accountId: String(fill.accountId),
      ...(fill.chatId !== null ? { chatId: String(fill.chatId) } : {}),
      ...failure,
      floodWaitS: null,
    })
    reported = true
  }
  const retired = params.journal.update(
    fill.ref,
    { status: "retired" },
    params.nowMs ?? Date.now(),
  )
  return { dryRun: false, fill: retired, reported }
}

/** A pending background member queued this long ago is reported stalled. */
export const BACKGROUND_STALL_MS = 24 * 60 * 60 * 1000

/**
 * The background queue as `pool status` shows it: pending members, the
 * next due time, and `stalled` when the oldest pending member has waited
 * `BACKGROUND_STALL_MS` or more. Rows not yet due never block filling.
 */
export function backgroundView(
  queue: Pick<BackgroundQueue, "summary" | "oldestPendingMs">,
  nowMs: number,
): {
  pending: number
  nextAt: string | null
  oldestPendingAt: string | null
  stalled: boolean
} {
  const { pending, nextAtMs } = queue.summary()
  const oldest = queue.oldestPendingMs()
  return {
    pending,
    nextAt: pending && nextAtMs !== null ? new Date(nextAtMs).toISOString() : null,
    oldestPendingAt: oldest === null ? null : new Date(oldest).toISOString(),
    stalled: oldest !== null && nowMs - oldest >= BACKGROUND_STALL_MS,
  }
}

/** `tg-sai pool status` and `tg-sai pool retire`. */
export function addPoolCommands(program: Command): void {
  const pool = program
    .command("pool")
    .description("Inspect and recover this host's deal chat pool fills")
  pool
    .command("status")
    .description("List recent pool fills from the local journal (read only)")
    .option("--account-id <id>", "Owner account", String(CUSTODY_USER_ID))
    .option("--limit <count>", "Fills to list", "20")
    .action((options: { accountId: string; limit: string }) => {
      const db = openStateDb(stateDbPath())
      const journal = new PoolJournal(db)
      try {
        const accountId = Number(options.accountId)
        console.log(
          JSON.stringify(
            {
              accountId: String(accountId),
              open: journal.open(accountId),
              background: backgroundView(new BackgroundQueue(db), Date.now()),
              nextFillAt: new Date(journal.nextFillMs(accountId)).toISOString(),
              fills: journal.list(accountId, Number(options.limit)),
            },
            null,
            2,
          ),
        )
      } finally {
        journal.close()
      }
    })
  pool
    .command("retire")
    .description(
      "Retire a failed or stuck pool fill so filling resumes; needs the worker stopped",
    )
    .requiredOption("--ref <ref>", "Pool ref (8 hex) from pool status")
    .option("--run", "Retire it; without this only prints the record")
    .action(async (options: { ref: string; run?: boolean }) => {
      // The worker holds this lock while it runs, so nothing races the write.
      await withAccountLock(
        join(configDir(), "provisioning", "worker-service.lock"),
        async () => {
          const journal = new PoolJournal(openStateDb(stateDbPath()))
          try {
            const result = await retirePoolFill({
              journal,
              cloud: new CliProvisionCloud(),
              ref: options.ref,
              run: Boolean(options.run),
            })
            console.log(JSON.stringify(result, null, 2))
          } finally {
            journal.close()
          }
        },
      )
    })
}
