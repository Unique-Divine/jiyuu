import type { Command } from "commander"
import { checkTelegramRuntimeHealth } from "./runtimeHealth"
import { activeTelemetryRun } from "./telemetry/run"

export function addRuntimeHealthCommands(program: Command, legacyLocal?: () => Promise<void>): void {
  const health = program.command("health")
    .description("Check Telegram runtime readiness; defaults to offline crypto/compression checks")
    .option("--offline", "Initialize the actual Telegram runtime without credentials or network access")
    .option("--online", "Also verify custody getMe under the account lock; no Telegram writes")
    .option("--custody-profile <name>", "Explicit saved custody profile for user 8274304175")
    .option("--timeout-ms <ms>", "Online identity timeout, 1000 through 60000", "20000")
    .option("--json", "Print a machine-readable readiness result")
    .addHelpText("after", `
Examples:
  tg-sai health
  tg-sai health --offline --json
  tg-sai health --online --custody-profile SaiTeam_AI --json

Offline success proves runtime initialization, not Telegram authentication.
Online mode checks the custody account only and refuses personal-profile fallback.
`)
  if (legacyLocal)
    health.option("--local", "Run the earlier active-profile and workstation diagnostics, including getMe")
  health.action(async (options: { offline?: boolean; online?: boolean;
    custodyProfile?: string; timeoutMs: string; json?: boolean; local?: boolean }) => {
    if (options.offline && options.online)
      throw new Error("Choose --offline or --online")
    if (options.local) {
      if (options.offline || options.online || options.custodyProfile || options.json)
        throw new Error("--local cannot be combined with runtime health options")
      await legacyLocal!()
      return
    }
    if (options.custodyProfile && !options.online)
      throw new Error("--custody-profile requires explicit --online")
    const result = await checkTelegramRuntimeHealth({ online: options.online,
      custodyProfile: options.custodyProfile, timeoutMs: Number(options.timeoutMs),
      telemetry: activeTelemetryRun()?.client() })
    if (options.json) console.log(JSON.stringify(result))
    else {
      console.log(`mode: ${result.mode}`)
      for (const check of result.checks)
        console.log(`${check.name}: ${check.ok ? "ok" : check.reason ?? "failed"}`)
      console.log(`ready: ${result.ok}`)
    }
    process.exitCode = result.ok ? 0 : 1
  })
}
