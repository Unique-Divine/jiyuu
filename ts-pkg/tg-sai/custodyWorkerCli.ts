import type { Command } from "commander"
import { runCustodyWorker } from "./custodyWorker"

/** Root main.ts calls this once beside addProvisionCommands. No MCP is involved. */
export function addCustodyWorkerCommands(
  program: Command,
  run: typeof runCustodyWorker = runCustodyWorker,
): void {
  program
    .command("worker")
    .description(
      "Run the custody-account provisioning queue; defaults to a read-only one-page preview",
    )
    .requiredOption(
      "--custody-profile <name>",
      "Explicit saved custody profile for user 8274304175",
    )
    .option("--once", "Process one bounded page and exit")
    .option("--run", "Execute approved queued Telegram setup jobs")
    .option("--limit <count>", "Maximum jobs per tick, 1 through 50", "10")
    .option(
      "--interval-ms <ms>",
      "Polling interval, 1000 through 60000",
      "5000",
    )
    .action(
      async (options: {
        custodyProfile: string
        once?: boolean
        run?: boolean
        limit: string
        intervalMs: string
      }) => {
        const controller = new AbortController()
        const stop = () => controller.abort()
        process.once("SIGTERM", stop)
        process.once("SIGINT", stop)
        try {
          await run({
            ...options,
            once: options.once || !options.run,
            limit: Number(options.limit),
            intervalMs: Number(options.intervalMs),
            signal: controller.signal,
          })
        } finally {
          process.removeListener("SIGTERM", stop)
          process.removeListener("SIGINT", stop)
        }
      },
    )
}
