import { join } from "node:path"
import type { Command } from "commander"
import { CUSTODY_USER_ID } from "./custodyWorker"
import type { PoolCloud } from "./poolFiller"
import { configDir } from "./profiles"
import { CliProvisionCloud } from "./provisioningCloud"
import { withAccountLock } from "./provisioningRuntime"
import { openStateDb, stateDbPath } from "./state/db"
import { PoolJournal } from "./state/pool"
import type { PoolFill } from "./state/pool"

/**
 * Retires the open pool fill `ref`, so the filler starts new fills again.
 * A fill that is still `active` and has a chat is reported to Deal Desk
 * with `pool-fail` (stage `other`, error `operator_retired`), which retires
 * the chat there; a `failed` one was reported when it failed. The chat
 * itself is left as it is. Without `run` it only returns the record.
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
  if (fill.status === "active" && fill.chatId !== null) {
    await params.cloud.poolFail({
      accountId: String(fill.accountId),
      chatId: String(fill.chatId),
      stage: "other",
      error: "operator_retired",
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
      const journal = new PoolJournal(openStateDb(stateDbPath()))
      try {
        const accountId = Number(options.accountId)
        console.log(
          JSON.stringify(
            {
              accountId: String(accountId),
              open: journal.open(accountId),
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
