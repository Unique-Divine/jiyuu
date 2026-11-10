import { afterAll, describe, expect, test } from "bun:test"
import { chmod, lstat, mkdir, mkdtemp, readdir, readFile, rename, rm, stat, symlink, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { filesContaining, scrubPasswordText } from "./passwordMigration"
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
  await mkdir(storeDir, { recursive: true })
  await writeFile(join(storeDir, ".gpg-id"), "fake-gpg-id\n")
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
    join(stateDir, "backups", "peers.json"),
    '{"peers":[{"accessHash":9007199254740993123}],"password":"' + ALPHA + '"}',
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

describe("scrubPasswordText", () => {
  test("removes password keys at any depth and counts non-empty ones", () => {
    const scrubbed = scrubPasswordText(JSON.stringify({
      profiles: [{ name: "a", password: "x" }, { name: "b", password: "" }],
      env: { TELEGRAM_PASSWORD: "y", TELEGRAM_API_ID: "1" },
      list: [],
      empty: {},
    }))
    expect(scrubbed.removed).toBe(2)
    expect(JSON.parse(scrubbed.text)).toEqual({
      profiles: [{ name: "a" }, { name: "b" }],
      env: { TELEGRAM_API_ID: "1" },
      list: [],
      empty: {},
    })
  })

  test("keeps 64-bit integers and string escapes exactly as written", () => {
    const text =
      '{"peers":[{"accessHash":9007199254740993123,"id":-1002233445566778899}],' +
      '"title":"a\\"b\\u00e9","password":"fake-2fa","n":1.50e3}'
    const scrubbed = scrubPasswordText(text)
    expect(scrubbed.removed).toBe(1)
    expect(scrubbed.text).toContain('"accessHash": 9007199254740993123')
    expect(scrubbed.text).toContain('"id": -1002233445566778899')
    expect(scrubbed.text).toContain('"title": "a\\"b\\u00e9"')
    expect(scrubbed.text).toContain('"n": 1.50e3')
    expect(scrubbed.text).not.toContain("fake-2fa")
  })

  test("pretty output matches JSON.stringify for safe values", () => {
    const value = { a: [1, { b: true, c: null }], d: "x" }
    expect(scrubPasswordText(JSON.stringify(value)).text).toBe(
      `${JSON.stringify(value, null, 2)}\n`,
    )
  })

  test("rejects text that is not JSON", () => {
    expect(() => scrubPasswordText("not json")).toThrow(SyntaxError)
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
      const peers = await readFile(join(f.stateDir, "backups", "peers.json"), "utf8")
      expect(peers).toContain("9007199254740993123")
      expect(peers).not.toContain("password")
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

      // A rerun still finds the copy, now through the profile's pass entry.
      const rerun = runCli(f, ["profile", "migrate-password", "--run"])
      expect(rerun.exitCode).toBe(1)
      expectNoSecrets(rerun)
      expect(rerun.stdout).toContain("profiles: none with a plain-text password")
      expect(rerun.stdout).toContain(
        "residual: .env still contains a migrated password",
      )

      await rm(join(f.stateDir, ".env"))
      const clean = runCli(f, ["profile", "migrate-password", "--run"])
      expect(clean.exitCode).toBe(0)
      expect(clean.stdout).toContain("residual_files: 0")
    } finally {
      await f.cleanup()
    }
  })

  test("--run stops before any change when pass is not initialised", async () => {
    const f = await fixture()
    try {
      await rm(join(f.storeDir, ".gpg-id"))
      const before = await snapshot(f.stateDir)
      const result = runCli(f, ["profile", "migrate-password", "--run"])
      expect(result.exitCode).not.toBe(0)
      expectNoSecrets(result)
      expect(result.stderr).toContain("is not initialised")
      // Telemetry may record the run; credential files stay unchanged.
      const after = await snapshot(f.stateDir)
      for (const name of Object.keys(before).filter((n) => n.endsWith(".json"))) {
        expect(after[name]).toBe(before[name]!)
      }
    } finally {
      await f.cleanup()
    }
  })

  test("--run writes through a symlinked profiles.json and scans symlinked files", async () => {
    const f = await fixture()
    try {
      const outside = join(f.stateDir, "..", "outside")
      await mkdir(outside)
      await rename(join(f.stateDir, "profiles.json"), join(outside, "real.json"))
      await symlink("../outside/real.json", join(f.stateDir, "profiles.json"))
      await writeFile(join(outside, "env"), `TELEGRAM_PASSWORD=${BETA}\n`)
      await symlink(join(outside, "env"), join(f.stateDir, ".env"))
      await mkdir(join(outside, "logs"))
      await symlink(join(outside, "logs"), join(f.stateDir, "linked-logs"))

      const result = runCli(f, ["profile", "migrate-password", "--run"])
      expectNoSecrets(result)
      expect((await lstat(join(f.stateDir, "profiles.json"))).isSymbolicLink()).toBe(true)
      const real = await readFile(join(outside, "real.json"), "utf8")
      expect(real).not.toContain(ALPHA)
      expect(real).toContain("passwordPassEntry")
      expect(result.stdout).toContain("residual: .env still contains a migrated password")
      expect(result.stdout).toContain(
        "residual: linked-logs is a symlink the scan does not follow; check its target",
      )
      expect(result.stdout).toContain("residual_files: 2")
      expect(result.exitCode).toBe(1)
    } finally {
      await f.cleanup()
    }
  })

  test("--run leaves symlinked backups and their targets untouched", async () => {
    const f = await fixture()
    try {
      const other = join(f.stateDir, "..", "otherapp")
      await mkdir(other)
      const body = '{"db":{"password":"unrelated-db-pw"},"port":9007199254740993123}'
      await writeFile(join(other, "config.json"), body, { mode: 0o644 })
      await chmod(join(other, "config.json"), 0o644)
      await symlink("../../otherapp/config.json", join(f.stateDir, "backups", "linked.json"))

      const result = runCli(f, ["profile", "migrate-password", "--run"])
      expectNoSecrets(result)
      expect(result.stdout).toContain(
        "backup: backups/linked.json is a symlink; not changed, check its target by hand",
      )
      expect(result.exitCode).toBe(1)
      expect(await readFile(join(other, "config.json"), "utf8")).toBe(body)
      expect((await stat(join(other, "config.json"))).mode & 0o777).toBe(0o644)
      expect((await lstat(join(f.stateDir, "backups", "linked.json"))).isSymbolicLink()).toBe(true)
    } finally {
      await f.cleanup()
    }
  })

  test("--profile limits the scan to that profile's pass entry", async () => {
    const f = await fixture()
    try {
      expect(runCli(f, ["profile", "migrate-password", "--profile", "beta", "--run"]).exitCode).toBe(0)
      await rm(join(f.storeDir, ".calls"))
      const result = runCli(f, ["profile", "migrate-password", "--profile", "alpha", "--run"])
      expect(result.exitCode).toBe(0)
      expectNoSecrets(result)
      const calls = await readFile(join(f.storeDir, ".calls"), "utf8")
      expect(calls).toContain("tg-sai/alpha/telegram-2fa")
      expect(calls).not.toContain("tg-sai/beta")
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
