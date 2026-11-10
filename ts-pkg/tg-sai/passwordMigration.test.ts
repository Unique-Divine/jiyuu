import { afterAll, describe, expect, test } from "bun:test"
import { mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { filesContaining, scrubPasswordFields } from "./passwordMigration"
import { installFakePass } from "./testdata"

const packageRoot = import.meta.dir
const fakePass = installFakePass()
afterAll(async () => {
  await rm(dirname(fakePass.binDir), { recursive: true, force: true })
})

const ALPHA = "fake-2fa-alpha-7Q"
const BETA = "fake-2fa-beta-9Z"
const SECRETS = [ALPHA, BETA]

interface Fixture {
  stateDir: string
  storeDir: string
  cleanup(): Promise<void>
}

function profileRow(name: string, userId: number, password: string) {
  return {
    name,
    apiId: 123,
    apiHash: `hash-${name}`,
    sessionString: `session-${name}`,
    password,
    handle: `${name}_handle`,
    userId,
    displayName: name,
    createdAt: "2026-01-01T00:00:00.000Z",
    updatedAt: "2026-01-01T00:00:00.000Z",
  }
}

async function fixture(): Promise<Fixture> {
  const root = await mkdtemp(join(tmpdir(), "tg-sai-migrate-"))
  const stateDir = join(root, "state")
  const storeDir = join(root, "store")
  await mkdir(join(stateDir, "backups"), { recursive: true })
  const profiles = {
    profiles: [profileRow("alpha", 111, ALPHA), profileRow("beta", 222, BETA)],
  }
  await writeFile(join(stateDir, "profiles.json"), JSON.stringify(profiles), {
    mode: 0o600,
  })
  await writeFile(
    join(stateDir, "backups", "profiles-1.json"),
    JSON.stringify(profiles),
    { mode: 0o600 },
  )
  await writeFile(
    join(stateDir, "backups", "env-copy.json"),
    JSON.stringify({ nested: { env: { TELEGRAM_PASSWORD: BETA, keep: 1 } } }),
    { mode: 0o600 },
  )
  await writeFile(
    join(stateDir, "backups", "clean.json"),
    JSON.stringify({ profiles: [{ name: "x", password: "" }] }),
  )
  return { stateDir, storeDir, cleanup: () => rm(root, { recursive: true, force: true }) }
}

interface RunResult {
  exitCode: number
  stdout: string
  stderr: string
}

function runCli(f: Fixture, args: string[]): RunResult {
  const result = Bun.spawnSync({
    cmd: ["bun", "main.ts", ...args],
    cwd: packageRoot,
    stdin: "ignore",
    env: {
      ...process.env,
      PATH: fakePass.path,
      PASSWORD_STORE_DIR: f.storeDir,
      TG_SAI_DIR: f.stateDir,
      TG_SAI_REPO_DIR: join(f.stateDir, "..", "repo"),
      HOME: join(f.stateDir, "..", "home"),
      TELEGRAM_API_ID: "",
      TELEGRAM_API_HASH: "",
      TELEGRAM_SESSION_STRING: "",
      TELEGRAM_PASSWORD: "",
    },
    stdout: "pipe",
    stderr: "pipe",
  })
  return {
    exitCode: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  }
}

function expectNoSecrets(result: RunResult): void {
  for (const secret of SECRETS) {
    expect(result.stdout).not.toContain(secret)
    expect(result.stderr).not.toContain(secret)
  }
}

async function snapshot(dir: string): Promise<Record<string, string>> {
  const out: Record<string, string> = {}
  for (const name of await readdir(dir, { recursive: true })) {
    const path = join(dir, name)
    if ((await stat(path)).isFile()) out[name] = await readFile(path, "utf8")
  }
  return out
}

describe("scrubPasswordFields", () => {
  test("removes password keys at any depth and counts non-empty ones", () => {
    const scrubbed = scrubPasswordFields({
      profiles: [{ name: "a", password: "x" }, { name: "b", password: "" }],
      env: { TELEGRAM_PASSWORD: "y", TELEGRAM_API_ID: "1" },
    })
    expect(scrubbed.removed).toBe(2)
    expect(scrubbed.value).toEqual({
      profiles: [{ name: "a" }, { name: "b" }],
      env: { TELEGRAM_API_ID: "1" },
    })
  })
})

describe("tg-sai profile migrate-password", () => {
  test("dry run changes nothing and never runs pass", async () => {
    const f = await fixture()
    try {
      const before = await snapshot(f.stateDir)
      const result = runCli(f, ["profile", "migrate-password"])
      expect(result.exitCode).toBe(0)
      expectNoSecrets(result)
      expect(result.stdout).toContain("mode: dry_run")
      expect(result.stdout).toContain("profile: alpha")
      expect(result.stdout).toContain("  pass_entry: tg-sai/alpha/telegram-2fa")
      expect(result.stdout).toContain("  pass: insert entry")
      expect(result.stdout).toContain("backup: backups/profiles-1.json remove 2 password field(s)")
      expect(result.stdout).toContain("backup: backups/env-copy.json remove 1 password field(s)")
      expect(result.stdout).toContain("backup: backups/clean.json no password fields")
      expect(result.stdout).toContain("next: rerun with --run to apply")
      const after = await snapshot(f.stateDir)
      for (const name of Object.keys(before)) {
        expect(after[name]).toBe(before[name]!)
      }
      expect(await Bun.file(join(f.storeDir, ".calls")).exists()).toBe(false)
    } finally {
      await f.cleanup()
    }
  })

  test("--run moves passwords to pass and leaves none under the state dir", async () => {
    const f = await fixture()
    try {
      const result = runCli(f, ["profile", "migrate-password", "--run"])
      expect(result.stderr).toBe("")
      expect(result.exitCode).toBe(0)
      expectNoSecrets(result)
      expect(result.stdout).toContain("  pass: inserted and read back")
      expect(result.stdout).toContain("residual_files: 0")

      expect(
        await readFile(join(f.storeDir, "tg-sai/alpha/telegram-2fa.gpg"), "utf8"),
      ).toBe(`${ALPHA}\n`)
      expect(
        await readFile(join(f.storeDir, "tg-sai/beta/telegram-2fa.gpg"), "utf8"),
      ).toBe(`${BETA}\n`)

      const config = JSON.parse(
        await readFile(join(f.stateDir, "profiles.json"), "utf8"),
      )
      for (const profile of config.profiles) {
        expect("password" in profile).toBe(false)
        expect(profile.passwordPassEntry).toBe(
          `tg-sai/${profile.name}/telegram-2fa`,
        )
        expect(profile.sessionString).toBe(`session-${profile.name}`)
      }
      const backup = JSON.parse(
        await readFile(join(f.stateDir, "backups", "profiles-1.json"), "utf8"),
      )
      expect(backup.profiles[0].sessionString).toBe("session-alpha")
      expect((await stat(join(f.stateDir, "backups", "env-copy.json"))).mode & 0o777).toBe(0o600)

      // Later commands print no password and keep the state dir clean,
      // including telemetry and state databases they write.
      const followUps = [
        runCli(f, ["profile", "show", "alpha"]),
        runCli(f, ["profile", "list"]),
        runCli(f, ["profile", "migrate-password", "--run"]),
      ]
      for (const followUp of followUps) {
        expect(followUp.exitCode).toBe(0)
        expectNoSecrets(followUp)
      }
      expect(followUps[0]!.stdout).toContain('"passwordStorage": "pass"')
      expect(followUps[0]!.stderr).toBe("")
      expect(followUps[2]!.stdout).toContain("profiles: none with a plain-text password")
      expect(await filesContaining(f.stateDir, SECRETS)).toEqual([])
    } finally {
      await f.cleanup()
    }
  })

  test("--profile and --pass-entry migrate one profile", async () => {
    const f = await fixture()
    try {
      const result = runCli(f, [
        "profile", "migrate-password", "--profile", "beta_handle",
        "--pass-entry", "custom/beta-2fa", "--run",
      ])
      expect(result.exitCode).toBe(0)
      expectNoSecrets(result)
      expect(result.stdout).not.toContain("profile: alpha")
      expect(result.stdout).toContain("residual_files: 0")
      expect(await filesContaining(f.stateDir, [BETA])).toEqual([])
      expect(await readFile(join(f.storeDir, "custom/beta-2fa.gpg"), "utf8")).toBe(
        `${BETA}\n`,
      )
      const config = JSON.parse(
        await readFile(join(f.stateDir, "profiles.json"), "utf8"),
      )
      expect(config.profiles[0].password).toBe(ALPHA)
      expect(config.profiles[1].passwordPassEntry).toBe("custom/beta-2fa")
      expect("password" in config.profiles[1]).toBe(false)

      const show = runCli(f, ["profile", "show", "alpha"])
      expectNoSecrets(show)
      expect(show.stdout).toContain('"passwordStorage": "plain_text"')
      expect(show.stderr).toContain(
        "warning: profile alpha keeps its 2FA password in profiles.json",
      )
    } finally {
      await f.cleanup()
    }
  })

  test("refuses a pass entry that holds a different password", async () => {
    const f = await fixture()
    try {
      await mkdir(join(f.storeDir, "tg-sai", "alpha"), { recursive: true })
      await writeFile(join(f.storeDir, "tg-sai/alpha/telegram-2fa.gpg"), "other-value\n")
      const before = await readFile(join(f.stateDir, "profiles.json"), "utf8")
      const result = runCli(f, ["profile", "migrate-password", "--run"])
      expect(result.exitCode).not.toBe(0)
      expectNoSecrets(result)
      expect(result.stderr).toContain(
        "pass entry tg-sai/alpha/telegram-2fa already holds a different password",
      )
      expect(result.stderr).not.toContain("other-value")
      expect(await readFile(join(f.stateDir, "profiles.json"), "utf8")).toBe(before)
    } finally {
      await f.cleanup()
    }
  })

  test("--run reports other files that still hold a migrated password", async () => {
    const f = await fixture()
    try {
      await writeFile(join(f.stateDir, ".env"), `TELEGRAM_PASSWORD=${ALPHA}\n`)
      const result = runCli(f, ["profile", "migrate-password", "--run"])
      expect(result.exitCode).toBe(1)
      expectNoSecrets(result)
      expect(result.stdout).toContain(
        "residual: .env still contains a migrated password",
      )
      expect(result.stdout).toContain("residual_files: 1")
    } finally {
      await f.cleanup()
    }
  })

  test("--pass-entry needs --profile", async () => {
    const f = await fixture()
    try {
      const result = runCli(f, ["profile", "migrate-password", "--pass-entry", "x"])
      expect(result.exitCode).not.toBe(0)
      expect(result.stderr).toContain("--pass-entry needs --profile")
    } finally {
      await f.cleanup()
    }
  })
})
