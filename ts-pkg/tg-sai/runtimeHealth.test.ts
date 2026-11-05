import { expect, test } from "bun:test"
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises"
import { join } from "node:path"
import { tmpdir } from "node:os"
import { checkOfflineTelegramRuntime } from "./runtimeHealth"

async function output(command: string[], options: { cwd?: string; env?: NodeJS.ProcessEnv } = {}) {
  const child = Bun.spawn(command, { ...options, stdout: "pipe", stderr: "pipe" })
  return { code: await child.exited, stdout: await new Response(child.stdout).text(),
    stderr: await new Response(child.stderr).text() }
}

test("offline health exercises real crypto, compression, storage and client preparation", async () => {
  const result = await checkOfflineTelegramRuntime()
  expect(result.ok).toBe(true)
  expect(result.mode).toBe("offline")
  expect(result.checks.map(check => check.name)).toEqual([
    "telegram_runtime_assets", "telegram_crypto", "telegram_compression", "telegram_client_prepare",
  ])
})

test("compiled health runs with only the binary in an empty working directory", async () => {
  const directory = await mkdtemp(join(tmpdir(), "tg-sai-compiled-runtime-"))
  try {
    const binary = join(directory, "tg-sai-health")
    const entry = join(directory, "entry.ts")
    const empty = join(directory, "empty")
    await mkdir(empty)
    await writeFile(entry, `import {Command} from ${JSON.stringify(join(import.meta.dir, "node_modules/commander/index.js"))};\nimport {addRuntimeHealthCommands} from ${JSON.stringify(join(import.meta.dir, "runtimeHealthCli.ts"))};\nconst p=new Command().name('tg-sai');addRuntimeHealthCommands(p);await p.parseAsync(process.argv);\n`)
    const build = await output([process.execPath, "build", "--compile", entry, "--outfile", binary])
    expect(build.code).toBe(0)
    await rm(entry)
    const result = await output([binary, "health", "--offline", "--json"], {
      cwd: empty, env: { ...process.env, HOME: empty, TG_SAI_DIR: join(empty, "missing"), NODE_PATH: "" },
    })
    expect(result.code).toBe(0)
    expect(result.stderr).toBe("")
    const value = JSON.parse(result.stdout)
    expect(value.ok).toBe(true)
    expect(value.checks.find((c: { name: string }) => c.name === "telegram_client_prepare")).toEqual({ name: "telegram_client_prepare", ok: true })
    expect(value).not.toHaveProperty("userId")
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
}, 30000)

test("online health requires an explicit custody profile and refuses a personal identity", async () => {
  const directory = await mkdtemp(join(tmpdir(), "tg-sai-online-health-"))
  const privateText = "PRIVATE_SESSION_MUST_NOT_APPEAR"
  try {
    await writeFile(join(directory, "profiles.json"), JSON.stringify({ profiles: [{
      name: "personal", apiId: 1, apiHash: privateText, sessionString: privateText,
      userId: 1912700300, handle: "uniquedivine",
    }] }))
    const env = { ...process.env, TG_SAI_DIR: directory }
    const noProfile = await output([process.execPath, join(import.meta.dir, "main.ts"), "health", "--online", "--json"], { env })
    expect(noProfile.code).toBe(1)
    expect(JSON.parse(noProfile.stdout).checks.at(-1).reason).toBe("explicit_custody_profile_required")
    const personal = await output([process.execPath, join(import.meta.dir, "main.ts"), "health", "--online", "--custody-profile", "personal", "--json"], { env })
    expect(personal.code).toBe(1)
    expect(JSON.parse(personal.stdout).ok).toBe(false)
    expect(personal.stdout + personal.stderr).not.toContain(privateText)
    expect(JSON.parse(personal.stdout).checks.at(-1).name).toBe("custody_identity")
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
