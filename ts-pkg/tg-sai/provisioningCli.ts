import { randomUUID } from "node:crypto"
import { readFile } from "node:fs/promises"
import { hostname } from "node:os"
import type { Command } from "commander"
import {
  backgroundEnqueue,
  openBackgroundQueue,
  recordingMemberAdds,
} from "./backgroundWorker"
import type { BackgroundQueue } from "./state/background"
import {
  creationMarker,
  provisionSetup,
  requestFingerprint,
  validateRequest,
} from "./provisioning"
import type { ProvisionRequest } from "./provisioning"
import { CliProvisionCloud } from "./provisioningCloud"
import { rewriteChatDescription } from "./provisioningDescription"
import {
  FileProvisionJournal,
  withProvisionTelegram,
} from "./provisioningRuntime"
import { activeTelemetryRun } from "./telemetry/run"

/** Telegram telemetry for this CLI run, if the entry point started one. */
function cliTelemetry() {
  return activeTelemetryRun()?.client()
}

function print(value: unknown): void {
  console.log(
    JSON.stringify(
      value,
      (key, item) => (key === "claimToken" ? undefined : item),
      2,
    ),
  )
}
function chatId(value: string): number {
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed) || parsed >= 0)
    throw new Error("Explicit negative Telegram supergroup ID required")
  return parsed
}
export function addProvisionCommands(program: Command): void {
  const provision = program
    .command("chat")
    .description("Sai deal chat operations")
    .command("provision")
    .description(
      "Prepare and resume custody-account setup through Deal Desk cloud jobs",
    )
  provision
    .command("prepare")
    .requiredOption("--file <path>", "Provisioning request JSON file")
    .option("--run", "Persist the request to authoritative cloud state")
    .action(async (options: { file: string; run?: boolean }) => {
      const raw = JSON.parse(await readFile(options.file, "utf8"))
      const request = validateRequest({
        ...raw,
        expectedUserId: Number(raw.expectedUserId),
      } as ProvisionRequest)
      if (!options.run)
        return print({
          dryRun: true,
          request,
          fingerprint: requestFingerprint(request),
        })
      print(await new CliProvisionCloud().prepare(request))
    })
  provision
    .command("status")
    .requiredOption("--request-id <id>", "Stable setup request ID")
    .action(async (options: { requestId: string }) =>
      print(await new CliProvisionCloud().view(options.requestId)),
    )
  for (const name of ["run", "resume"]) {
    provision
      .command(name)
      .requiredOption("--request-id <id>", "Stable setup request ID")
      .option(
        "--run",
        "Execute Telegram setup writes and authoritative cloud progress",
      )
      .action(async (options: { requestId: string; run?: boolean }) => {
        const cloud = new CliProvisionCloud()
        const setup = await cloud.view(options.requestId)
        if (!options.run)
          return print({
            dryRun: true,
            setup,
            next: "Pass --run to execute this request with its explicit custody profile",
          })
        // Optional members need the queue; without them it only logs
        // this run's adds toward the account's adds today, best effort.
        let queue: BackgroundQueue | null = null
        if (setup.optionalParticipants?.length) queue = openBackgroundQueue()
        else
          try {
            queue = openBackgroundQueue()
          } catch {
            queue = null
          }
        try {
          print(
            await withProvisionTelegram(setup, (telegram) =>
              provisionSetup({
                setup,
                telegram: queue
                  ? recordingMemberAdds(telegram, queue)
                  : telegram,
                cloud,
                journal: new FileProvisionJournal(),
                workerId: `${hostname()}:${process.pid}:${randomUUID()}`,
                // The worker adds queued optional members later.
                ...(queue && setup.optionalParticipants?.length
                  ? { background: backgroundEnqueue(queue) }
                  : {}),
              }),
              cliTelemetry(),
            ),
          )
        } finally {
          queue?.close()
        }
      })
  }
  provision
    .command("resolve")
    .requiredOption("--request-id <id>", "Stable setup request ID")
    .requiredOption("--chat <id>", "Exact created supergroup ID", chatId)
    .requiredOption(
      "--reason <text>",
      "Operator evidence for explicit adoption",
    )
    .option("--run", "Verify and adopt the exact group in cloud state")
    .action(
      async (options: {
        requestId: string
        chat: number
        reason: string
        run?: boolean
      }) => {
        const cloud = new CliProvisionCloud()
        const setup = await cloud.view(options.requestId)
        if (!options.reason.trim())
          throw new Error("Resolution evidence required")
        if (!options.run)
          return print({
            dryRun: true,
            requestId: setup.requestId,
            chatId: options.chat,
            reason: options.reason,
          })
        if (!["creating", "creation_unknown"].includes(setup.state))
          throw new Error("Only an unresolved creation can be adopted")
        print(
          await withProvisionTelegram(setup, async (telegram) => {
            if (
              !(await telegram.verifyMarker(
                options.chat,
                creationMarker(setup),
              ))
            )
              throw new Error(
                "Resolution chat does not match custody owner and request marker",
              )
            return cloud.resolve(setup.requestId, options.chat, options.reason)
          }, cliTelemetry()),
        )
      },
    )
  provision
    .command("rewrite-description")
    .description(
      "Replace a chat's legacy sai-setup marker description with the " +
        "friendly description and short ref",
    )
    .requiredOption("--request-id <id>", "Stable setup request ID")
    .option("--run", "Write the new description to the chat")
    .action(async (options: { requestId: string; run?: boolean }) => {
      const setup = await new CliProvisionCloud().view(options.requestId)
      const local = await new FileProvisionJournal().read(setup.requestId)
      // Reads the chat even on a dry run, so the operator sees the change.
      print(
        await withProvisionTelegram(
          setup,
          (telegram) =>
            rewriteChatDescription({
              setup,
              local,
              telegram,
              run: Boolean(options.run),
            }),
          cliTelemetry(),
        ),
      )
    })
}
