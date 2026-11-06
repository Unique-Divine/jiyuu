import { describe, expect, test } from "bun:test"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

const packageRoot = import.meta.dir
const cliPath = "main.ts"

interface RunResult {
  exitCode: number
  stdout: string
  stderr: string
}

async function withTempConfig<T>(fn: (dir: string) => Promise<T>): Promise<T> {
  const dir = await mkdtemp(join(tmpdir(), "tg-sai-profile-test-"))
  try {
    return await fn(dir)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
}

function runCli(args: readonly string[], configDir: string, extraEnv: Record<string, string> = {}): RunResult {
  const result = Bun.spawnSync({
    cmd: ["bun", cliPath, ...args],
    cwd: packageRoot,
    env: {
      ...process.env,
      TG_SAI_DIR: configDir,
      TG_SAI_REPO_DIR: join(configDir, "repo"),
      HOME: join(configDir, "home"),
      TELEGRAM_API_ID: "",
      TELEGRAM_API_HASH: "",
      TELEGRAM_SESSION_STRING: "",
      TELEGRAM_PASSWORD: "",
      ...extraEnv,
    },
    stdout: "pipe",
    stderr: "pipe",
  })

  return {
    exitCode: result.exitCode,
    stdout: new TextDecoder().decode(result.stdout),
    stderr: new TextDecoder().decode(result.stderr),
  }
}

const addAliceArgs = [
  "profile",
  "add",
  "--name",
  "alice",
  "--api-id",
  "123",
  "--api-hash",
  "hash-alice",
  "--session-string",
  "session-alice",
  "--password",
  "password-alice",
  "--handle",
  "alice_handle",
  "--user-id",
  "111",
  "--display-name",
  "Alice Example",
]

const addBobArgs = [
  "profile",
  "add",
  "--name",
  "bob",
  "--api-id",
  "456",
  "--api-hash",
  "hash-bob",
  "--session-string",
  "session-bob",
  "--handle",
  "bob_handle",
  "--user-id",
  "222",
  "--display-name",
  "Bob Example",
]

describe("tg-sai profile CLI", () => {
  test.each([{ args: ["profile"] }, { args: ["profile", "add"] }])("bare %j displays help and empty profiles without saving or discovering credentials", async ({ args }) => {
    await withTempConfig(async (dir) => {
      await mkdir(join(dir, "home", ".cursor"), { recursive: true })
      await writeFile(join(dir, "home", ".cursor", "mcp.json"), "not valid JSON")
      const result = runCli(args, dir)
      expect(result.exitCode).toBe(0)
      expect(result.stderr).toBe("")
      expect(result.stdout).toContain(`Usage: tg-sai ${args.join(" ")}`)
      expect(result.stdout).toContain("Configured local profiles")
      expect(result.stdout).toContain("No profiles configured.")
      expect(await Bun.file(join(dir, "profiles.json")).exists()).toBe(false)
      if (args.length === 2) {
        expect(result.stdout).toContain("tg-sai profile add --name sai-custody --login --qr --interactive")
        expect(result.stdout).toContain("No profile is saved and no Telegram login starts without explicit options.")
      } else {
        expect(result.stdout.indexOf("Commands:")).toBeLessThan(result.stdout.indexOf("Configured local profiles"))
      }
    })
  })

  test.each([{ args: ["profile"] }, { args: ["profile", "add"] }])("bare %j lists existing profiles with no mutation or credential output", async ({ args }) => {
    await withTempConfig(async (dir) => {
      expect(runCli(addAliceArgs, dir).exitCode).toBe(0)
      expect(runCli(addBobArgs, dir).exitCode).toBe(0)
      const before = await readFile(join(dir, "profiles.json"), "utf8")
      const result = runCli(args, dir)
      expect(result.exitCode).toBe(0)
      expect(result.stderr).toBe("")
      expect(result.stdout).toContain("alice_handle")
      expect(result.stdout).toContain("bob_handle")
      expect(result.stdout).toContain("active/default")
      expect(await readFile(join(dir, "profiles.json"), "utf8")).toBe(before)
      for (const secret of ["hash-alice", "hash-bob", "session-alice", "session-bob", "password-alice"]) {
        expect(result.stdout).not.toContain(secret)
        expect(result.stderr).not.toContain(secret)
      }
      if (args.length === 2) {
        expect(result.stdout).toContain("Use a new --name to add a separate account.")
        expect(result.stdout).toContain("explicit options update the active profile.")
        expect(result.stdout).toContain("tg-sai profile show <selector>")
      }
    })
  })

  test.each(["--qr", "--phone", "--login"])("%s enrolls a fresh session without inheriting discovered account credentials", async (flag) => {
    await withTempConfig(async (dir) => {
      expect(runCli(addAliceArgs, dir).exitCode).toBe(0)
      const home = join(dir, "home")
      await mkdir(join(home, ".cursor"), { recursive: true })
      await writeFile(join(home, ".cursor", "mcp.json"), JSON.stringify({
        mcpServers: Object.fromEntries(["personal", "other"].map((name) => [name, {
          env: { TELEGRAM_API_ID: "123", TELEGRAM_API_HASH: "app-hash",
            TELEGRAM_SESSION_STRING: `${name}-session`, TELEGRAM_PASSWORD: `${name}-password` },
        }])),
      }))
      const bin = join(dir, "bin")
      await mkdir(bin)
      await writeFile(join(bin, "uv"), `#!/usr/bin/env bun
const args = process.argv.slice(2)
await Bun.write(args[args.indexOf("--session-output") + 1], "fresh-session")
await Bun.write(${JSON.stringify(join(dir, "generator-mode"))}, args.includes("--phone") ? "phone" : "qr")
`, { mode: 0o700 })
      const result = Bun.spawnSync({
        cmd: ["bun", cliPath, "profile", "add", "--name", "custody", flag,
          "--user-id", "666", "--display-name", "Custody fixture"],
        cwd: packageRoot,
        env: { ...process.env, HOME: home, TG_SAI_DIR: dir,
          TG_SAI_REPO_DIR: join(dir, "repo"), PATH: `${bin}:${process.env.PATH}`,
          TELEGRAM_API_ID: "", TELEGRAM_API_HASH: "",
          TELEGRAM_SESSION_STRING: "environment-personal-session",
          TELEGRAM_PASSWORD: "environment-personal-password" },
        stdout: "pipe", stderr: "pipe",
      })
      expect(result.exitCode).toBe(0)
      expect(await readFile(join(dir, "generator-mode"), "utf8")).toBe(flag === "--phone" ? "phone" : "qr")
      const config = JSON.parse(await readFile(join(dir, "profiles.json"), "utf8"))
      expect(config.profiles[0].name).toBe("alice")
      const custody = config.profiles.find((p: { name: string }) => p.name === "custody")
      expect(custody.sessionString).toBe("fresh-session")
      expect(custody.password).toBe("")
      expect(config.profiles[0].sessionString).toBe("session-alice")
      expect(result.stdout.toString()).not.toContain("fresh-session")
      expect(result.stderr.toString()).not.toContain("personal-session")
    })
  })

  test("fresh login rejects an explicitly supplied session", async () => {
    await withTempConfig(async (dir) => {
      const result = runCli([...addAliceArgs, "--qr"], dir)
      expect(result.exitCode).not.toBe(0)
      expect(result.stderr).toContain("Fresh login cannot be combined with --session-string")
      expect(runCli(["profile", "list"], dir).stdout).toContain("No profiles configured")
    })
  })

  test("prints help with exit code 0 when called without args", async () => {
    await withTempConfig(async (dir) => {
      const result = runCli([], dir)

      expect(result.exitCode).toBe(0)
      expect(result.stdout).toContain("Usage: tg-sai")
      expect(result.stdout).toContain("profile")
    })
  })

  test("lists empty profile config", async () => {
    await withTempConfig(async (dir) => {
      const result = runCli(["profile", "list"], dir)

      expect(result.exitCode).toBe(0)
      expect(result.stdout).toContain("No profiles configured.")
    })
  })

  test("profile remove without a selector shows empty-state help without creating a config", async () => {
    await withTempConfig(async (dir) => {
      const result = runCli(["profile", "remove"], dir)
      expect(result.exitCode).toBe(0)
      expect(result.stderr).toBe("")
      expect(result.stdout).toContain("Usage: tg-sai profile remove")
      expect(result.stdout).toContain("No saved profiles are available to remove.")
      expect(result.stdout).toContain("No profile is removed without an explicit selector.")
      expect(result.stdout).not.toContain("tg-sai profile remove -- ")
      expect(await Bun.file(join(dir, "profiles.json")).exists()).toBe(false)
    })
  })

  test("profile remove help generates executable examples for saved accounts and drafts without exposing credentials", async () => {
    await withTempConfig(async (dir) => {
      expect(runCli(addAliceArgs, dir).exitCode).toBe(0)
      expect(runCli(addBobArgs, dir).exitCode).toBe(0)
      expect(runCli(["profile", "add", "--name", "draft"], dir).exitCode).toBe(0)
      const before = await readFile(join(dir, "profiles.json"), "utf8")
      const result = runCli(["profile", "remove"], dir)
      expect(result.exitCode).toBe(0)
      expect(result.stderr).toBe("")
      expect(await readFile(join(dir, "profiles.json"), "utf8")).toBe(before)
      expect(result.stdout).toContain("not the Telegram account or its chat memberships")
      expect(result.stdout).toContain('tg-sai profile remove -- alice_handle  # "alice_handle" [active]')
      expect(result.stdout).toContain("tg-sai profile remove -- bob_handle")
      expect(result.stdout).toContain("tg-sai profile remove -- draft")
      for (const secret of ["hash-alice", "hash-bob", "session-alice", "session-bob", "password-alice"]) {
        expect(result.stdout).not.toContain(secret)
        expect(result.stderr).not.toContain(secret)
      }
      const selectors = [...result.stdout.matchAll(/^  tg-sai profile remove -- (\S+)  #/gm)].map((match) => match[1]!)
      expect(selectors).toEqual(["alice_handle", "bob_handle", "draft"])
      for (const [index, selector] of selectors.entries()) {
        await withTempConfig(async (copy) => {
          await writeFile(join(copy, "profiles.json"), before)
          const removed = runCli(["profile", "remove", "--", selector], copy)
          expect(removed.exitCode).toBe(0)
          const after = JSON.parse(await readFile(join(copy, "profiles.json"), "utf8"))
          expect(after.profiles.map((p: { name: string }) => p.name)).toEqual(
            ["alice", "bob", "draft"].filter((_, i) => i !== index),
          )
        })
      }
    })
  })

  test("profile remove examples resolve handle collisions and quote option-looking legacy names", async () => {
    await withTempConfig(async (dir) => {
      expect(runCli(addAliceArgs, dir).exitCode).toBe(0)
      expect(runCli(addBobArgs, dir).exitCode).toBe(0)
      const path = join(dir, "profiles.json")
      const config = JSON.parse(await readFile(path, "utf8"))
      config.profiles[1].handle = "alice_handle"
      const unusualName = "--old '$(touch SHOULD_NOT_EXIST)' profile"
      config.profiles.push({ ...config.profiles[0], name: unusualName, handle: null, userId: null })
      await writeFile(path, JSON.stringify(config))
      const before = await readFile(path, "utf8")
      const result = runCli(["profile", "remove"], dir)
      expect(result.exitCode).toBe(0)
      expect(result.stdout).toContain("tg-sai profile remove -- bob  #")
      const example = result.stdout.split("\n").find((line) => line.startsWith("  tg-sai profile remove -- '--old"))!
      expect(example).toBeDefined()
      expect(await readFile(path, "utf8")).toBe(before)
      const removed = Bun.spawnSync({
        cmd: ["sh", "-c", example.trim().replace(/^tg-sai /, "bun main.ts ")],
        cwd: packageRoot,
        env: { ...process.env, TG_SAI_DIR: dir },
        stdout: "pipe", stderr: "pipe",
      })
      expect(removed.exitCode).toBe(0)
      const after = JSON.parse(await readFile(path, "utf8"))
      expect(after.profiles.map((p: { name: string }) => p.name)).toEqual(["alice", "bob"])
      expect(await Bun.file(join(packageRoot, "SHOULD_NOT_EXIST")).exists()).toBe(false)
    })
  })

  test("adds, lists, shows, uses, and removes profiles", async () => {
    await withTempConfig(async (dir) => {
      const addAlice = runCli(addAliceArgs, dir)
      expect(addAlice.exitCode).toBe(0)
      expect(addAlice.stdout).toContain("profile_default: alice_handle")
      expect(addAlice.stdout).toContain("profile_index: 0")
      expect(addAlice.stdout).toContain("user_id: 111")
      expect(addAlice.stdout).toContain("telegram_password: stored")
      expect(addAlice.stdout).toContain(
        "ownership_transfer: profile password or TELEGRAM_PASSWORD enables transfer",
      )

      const listOne = runCli(["profile", "list"], dir)
      expect(listOne.exitCode).toBe(0)
      expect(listOne.stdout).toContain("* 0")
      expect(listOne.stdout).toContain("alice_handle")

      const addBob = runCli(addBobArgs, dir)
      expect(addBob.exitCode).toBe(0)
      expect(addBob.stdout).toContain("profile_saved: bob_handle")
      expect(addBob.stdout).toContain("profile_index: 1")

      const show = runCli(["profile", "show", "alice"], dir)
      expect(show.exitCode).toBe(0)
      expect(show.stdout).toContain('"name": "alice"')
      expect(show.stdout).toContain('"apiHash": "[redacted]"')
      expect(show.stdout).toContain('"password": "[redacted]"')
      expect(show.stdout).not.toContain("session-alice")
      expect(show.stdout).not.toContain("password-alice")

      const showByHandle = runCli(["profile", "show", "alice_handle"], dir)
      expect(showByHandle.exitCode).toBe(0)
      expect(showByHandle.stdout).toContain('"name": "alice"')

      const showByUserId = runCli(["profile", "show", "111"], dir)
      expect(showByUserId.exitCode).toBe(0)
      expect(showByUserId.stdout).toContain('"name": "alice"')

      const useBob = runCli(["profile", "use", "bob"], dir)
      expect(useBob.exitCode).toBe(0)
      expect(useBob.stdout).toContain("active_profile: bob_handle")

      const listTwo = runCli(["profile", "list"], dir)
      expect(listTwo.stdout).toContain("  1\talice_handle")
      expect(listTwo.stdout).toContain("* 0\tbob_handle")

      const removeBob = runCli(["profile", "remove", "bob"], dir)
      expect(removeBob.exitCode).toBe(0)
      expect(removeBob.stdout).toContain("removed_profile: bob")

      const listAfterRemove = runCli(["profile", "list"], dir)
      expect(listAfterRemove.stdout).toContain("* 0\talice_handle")
      expect(listAfterRemove.stdout).not.toContain("bob_handle")
    })
  })

  test.each(["bob", "bob_handle", "222", "1"])("password update by %s preserves a second account despite personal credential discovery", async (selector) => {
    await withTempConfig(async (dir) => {
      expect(runCli(addAliceArgs, dir).exitCode).toBe(0)
      expect(runCli(addBobArgs, dir).exitCode).toBe(0)
      const path = join(dir, "profiles.json")
      const before = JSON.parse(await readFile(path, "utf8"))
      const cursor = join(dir, "home", ".cursor")
      await mkdir(cursor, { recursive: true })
      await writeFile(join(cursor, "mcp.json"), JSON.stringify({ mcpServers: {
        first: { env: { TELEGRAM_API_ID: "123", TELEGRAM_API_HASH: "hash-alice", TELEGRAM_SESSION_STRING: "session-alice" } },
        second: { env: { TELEGRAM_API_ID: "987", TELEGRAM_API_HASH: "conflicting-hash", TELEGRAM_SESSION_STRING: "conflicting-session" } },
      } }))
      const result = runCli(["profile", "add", "--name", selector, "--password", "new-custody-password"], dir, {
        TELEGRAM_API_ID: "123", TELEGRAM_API_HASH: "hash-alice",
        TELEGRAM_SESSION_STRING: "session-alice", TELEGRAM_PASSWORD: "environment-password",
      })
      expect(result.exitCode).toBe(0)
      expect(result.stdout).toContain("profile_saved: bob_handle")
      expect(result.stdout).toContain("user_id: 222")
      const after = JSON.parse(await readFile(path, "utf8"))
      expect(after.profiles.length).toBe(2)
      expect(after.profiles[0]).toEqual(before.profiles[0])
      expect(after.profiles[1]).toEqual({ ...before.profiles[1], password: "new-custody-password", updatedAt: after.profiles[1].updatedAt })
      for (const secret of ["new-custody-password", "session-alice", "session-bob", "hash-alice", "hash-bob"]) {
        expect(result.stdout).not.toContain(secret)
        expect(result.stderr).not.toContain(secret)
      }
    })
  })

  test("password-only update of an unknown saved selector fails without importing another account", async () => {
    await withTempConfig(async (dir) => {
      expect(runCli(addAliceArgs, dir).exitCode).toBe(0)
      const path = join(dir, "profiles.json")
      const before = await readFile(path, "utf8")
      const result = runCli(["profile", "add", "--name", "missing-custody", "--password", "new-password"], dir, {
        TELEGRAM_API_ID: "123", TELEGRAM_API_HASH: "hash-alice", TELEGRAM_SESSION_STRING: "session-alice",
      })
      expect(result.exitCode).not.toBe(0)
      expect(result.stderr).toContain("Unknown configured profile")
      expect(await readFile(path, "utf8")).toBe(before)
    })
  })

  test("metadata updates keep saved identity and reject identity replacement without login", async () => {
    await withTempConfig(async (dir) => {
      expect(runCli(addAliceArgs, dir).exitCode).toBe(0)
      expect(runCli(addBobArgs, dir).exitCode).toBe(0)
      const path = join(dir, "profiles.json")
      const before = JSON.parse(await readFile(path, "utf8"))
      expect(runCli(["profile", "add", "--name", "bob_handle", "--display-name", "Updated label"], dir, {
        TELEGRAM_SESSION_STRING: "session-alice", TELEGRAM_API_ID: "123", TELEGRAM_API_HASH: "hash-alice",
      }).exitCode).toBe(0)
      const updated = JSON.parse(await readFile(path, "utf8"))
      expect(updated.profiles[0]).toEqual(before.profiles[0])
      expect(updated.profiles[1]).toEqual({ ...before.profiles[1], displayName: "Updated label", updatedAt: updated.profiles[1].updatedAt })
      const raw = await readFile(path, "utf8")
      const denied = runCli(["profile", "add", "--name", "bob", "--user-id", "111"], dir)
      expect(denied.exitCode).not.toBe(0)
      expect(denied.stderr).toContain("Changing a saved account identity requires")
      expect(await readFile(path, "utf8")).toBe(raw)
    })
  })

  test("session import cannot replace a known account and same-account refresh preserves its saved name", async () => {
    await withTempConfig(async (dir) => {
      expect(runCli(addAliceArgs, dir).exitCode).toBe(0)
      expect(runCli(addBobArgs, dir).exitCode).toBe(0)
      const path = join(dir, "profiles.json")
      const before = await readFile(path, "utf8")
      const imports = ["profile", "add", "--name", "bob_handle", "--api-id", "123", "--api-hash", "hash-alice",
        "--session-string", "session-alice", "--user-id", "111", "--display-name", "Alice Example"]
      const denied = runCli(imports, dir)
      expect(denied.exitCode).not.toBe(0)
      expect(denied.stderr).toContain("Login or import does not match the saved account")
      expect(await readFile(path, "utf8")).toBe(before)
      const refreshed = runCli(["profile", "add", "--name", "bob_handle", "--api-id", "456", "--api-hash", "hash-bob",
        "--session-string", "refreshed-bob-session", "--user-id", "222", "--display-name", "Bob Example"], dir)
      expect(refreshed.exitCode).toBe(0)
      const after = JSON.parse(await readFile(path, "utf8"))
      expect(after.profiles[0]).toEqual(JSON.parse(before).profiles[0])
      expect(after.profiles[1].name).toBe("bob")
      expect(after.profiles[1].userId).toBe(222)
      expect(after.profiles[1].sessionString).toBe("refreshed-bob-session")
    })
  })

  test("adds a password to an existing profile without clearing credentials", async () => {
    await withTempConfig(async (dir) => {
      const aliceArgsWithoutPassword = addAliceArgs.filter(
        (arg) => arg !== "--password" && arg !== "password-alice",
      )
      const addAlice = runCli(aliceArgsWithoutPassword, dir)
      expect(addAlice.exitCode).toBe(0)

      const addPassword = runCli(
        ["profile", "add", "--name", "alice", "--password", "password-alice"],
        dir,
      )
      expect(addPassword.exitCode).toBe(0)
      expect(addPassword.stdout).toContain("profile_default: alice_handle")
      expect(addPassword.stdout).toContain("telegram_password: stored")

      const show = runCli(["profile", "show", "alice"], dir)
      expect(show.exitCode).toBe(0)
      expect(show.stdout).toContain('"apiId": 123')
      expect(show.stdout).toContain('"apiHash": "[redacted]"')
      expect(show.stdout).toContain('"sessionString": "[redacted]"')
      expect(show.stdout).toContain('"password": "[redacted]"')
      expect(show.stdout).toContain('"handle": "alice_handle"')
      expect(show.stdout).toContain('"userId": 111')
      expect(show.stdout).toContain('"displayName": "Alice Example"')
      expect(show.stdout).not.toContain("session-alice")
      expect(show.stdout).not.toContain("password-alice")
    })
  })

  test("adds a draft profile with blank values", async () => {
    await withTempConfig(async (dir) => {
      const result = runCli(["profile", "add", "--name", "draft"], dir)

      expect(result.exitCode).toBe(0)
      expect(result.stdout).toContain("profile_default: draft")
      expect(result.stdout).toContain("profile_index: 0")
      expect(result.stdout).toContain("telegram_password: missing")

      const show = runCli(["profile", "show", "draft"], dir)
      expect(show.stdout).toContain('"apiId": null')
      expect(show.stdout).toContain('"password": ""')
      expect(show.stdout).toContain('"userId": null')
      expect(show.stdout).toContain('"displayName": ""')
    })
  })

  test("adds a draft profile from a partial discovery source", async () => {
    await withTempConfig(async (dir) => {
      const home = join(dir, "home")
      const discoveryPath = join(home, ".cursor", "mcp.json")
      await mkdir(join(home, ".cursor"), { recursive: true })
      await writeFile(
        discoveryPath,
        JSON.stringify({
          mcpServers: {
            telegram: {
              env: {
                TELEGRAM_API_ID: "789",
                TELEGRAM_API_HASH: "partial-hash",
              },
            },
          },
        }),
        "utf8",
      )

      const result = Bun.spawnSync({
        cmd: [
          "bun",
          cliPath,
          "profile",
          "add",
          "--name",
          "partial",
          "--src",
          "1",
        ],
        cwd: packageRoot,
        env: {
          ...process.env,
          HOME: home,
          TG_SAI_DIR: dir,
          TELEGRAM_API_ID: "",
          TELEGRAM_API_HASH: "",
          TELEGRAM_SESSION_STRING: "",
          TELEGRAM_PASSWORD: "",
        },
        stdout: "pipe",
        stderr: "pipe",
      })

      expect(result.exitCode).toBe(0)
      const show = runCli(["profile", "show", "partial"], dir)
      expect(show.stdout).toContain('"apiId": 789')
      expect(show.stdout).toContain('"apiHash": "[redacted]"')
      expect(show.stdout).toContain('"sessionString": ""')
    })
  })

  test("adds a profile from a numbered discovery source", async () => {
    await withTempConfig(async (dir) => {
      const home = join(dir, "home")
      const discoveryPath = join(home, ".cursor", "mcp.json")
      await mkdir(join(home, ".cursor"), { recursive: true })
      await writeFile(
        discoveryPath,
        JSON.stringify({
          mcpServers: {
            telegram: {
              env: {
                TELEGRAM_API_ID: "789",
                TELEGRAM_API_HASH: "discovered-hash",
                TELEGRAM_SESSION_STRING: "discovered-session",
              },
            },
          },
        }),
        "utf8",
      )

      const result = Bun.spawnSync({
        cmd: [
          "bun",
          cliPath,
          "profile",
          "add",
          "--name",
          "discovered",
          "--src",
          "1",
          "--handle",
          "discovered_handle",
          "--user-id",
          "333",
          "--display-name",
          "Discovered Example",
        ],
        cwd: packageRoot,
        env: {
          ...process.env,
          HOME: home,
          TG_SAI_DIR: dir,
          TELEGRAM_API_ID: "",
          TELEGRAM_API_HASH: "",
          TELEGRAM_SESSION_STRING: "",
          TELEGRAM_PASSWORD: "",
        },
        stdout: "pipe",
        stderr: "pipe",
      })

      const stdout = new TextDecoder().decode(result.stdout)
      expect(result.exitCode).toBe(0)
      expect(stdout).toContain("profile_default: discovered_handle")
      expect(stdout).toContain("telegram_password: missing")

      const show = runCli(["profile", "show", "discovered"], dir)
      expect(show.stdout).toContain('"apiId": 789')
      expect(show.stdout).not.toContain("discovered-session")
    })
  })

  test("adds unnamed default profile from non-conflicting discovered values", async () => {
    await withTempConfig(async (dir) => {
      const home = join(dir, "home")
      const userEnvPath = join(dir, ".env")
      const cursorPath = join(home, ".cursor", "mcp.json")
      await mkdir(join(home, ".cursor"), { recursive: true })
      await writeFile(
        userEnvPath,
        "TELEGRAM_SESSION_STRING=auto-session\nTELEGRAM_PASSWORD=auto-password\n",
        "utf8",
      )
      await writeFile(
        cursorPath,
        JSON.stringify({
          mcpServers: {
            telegram: {
              env: {
                TELEGRAM_API_ID: "654",
                TELEGRAM_API_HASH: "auto-hash",
              },
            },
          },
        }),
        "utf8",
      )

      const result = Bun.spawnSync({
        cmd: [
          "bun",
          cliPath,
          "profile",
          "add",
          "--handle",
          "auto_handle",
          "--user-id",
          "444",
          "--display-name",
          "Auto Example",
        ],
        cwd: packageRoot,
        env: {
          ...process.env,
          HOME: home,
          TG_SAI_DIR: dir,
          TELEGRAM_API_ID: "",
          TELEGRAM_API_HASH: "",
          TELEGRAM_SESSION_STRING: "",
          TELEGRAM_PASSWORD: "",
        },
        stdout: "pipe",
        stderr: "pipe",
      })

      const stdout = new TextDecoder().decode(result.stdout)
      expect(result.exitCode).toBe(0)
      expect(stdout).toContain("profile_default: auto_handle")
      expect(stdout).toContain("telegram_password: stored")

      const show = runCli(["profile", "show"], dir)
      expect(show.stdout).toContain('"apiId": 654')
      expect(show.stdout).toContain('"apiHash": "[redacted]"')
      expect(show.stdout).toContain('"sessionString": "[redacted]"')
      expect(show.stdout).toContain('"password": "[redacted]"')
    })
  })

  test("reports conflicting discovered values without --src", async () => {
    await withTempConfig(async (dir) => {
      const home = join(dir, "home")
      const repoDir = join(dir, "repo")
      const cursorPath = join(home, ".cursor", "mcp.json")
      await mkdir(join(home, ".cursor"), { recursive: true })
      await mkdir(repoDir, { recursive: true })
      await writeFile(join(repoDir, ".env"), "TELEGRAM_API_ID=111\n", "utf8")
      await writeFile(
        cursorPath,
        JSON.stringify({
          mcpServers: {
            telegram: {
              env: {
                TELEGRAM_API_ID: "222",
              },
            },
          },
        }),
        "utf8",
      )

      const result = Bun.spawnSync({
        cmd: ["bun", cliPath, "profile", "add", "--name", "conflict-test"],
        cwd: packageRoot,
        env: {
          ...process.env,
          HOME: home,
          TG_SAI_DIR: dir,
          TG_SAI_REPO_DIR: repoDir,
          TELEGRAM_API_ID: "",
          TELEGRAM_API_HASH: "",
          TELEGRAM_SESSION_STRING: "",
          TELEGRAM_PASSWORD: "",
        },
        stdout: "pipe",
        stderr: "pipe",
      })

      const stderr = new TextDecoder().decode(result.stderr)
      expect(result.exitCode).toBe(1)
      expect(stderr).toContain(
        "Conflicting Telegram credential values for TELEGRAM_API_ID",
      )
      expect(stderr).toContain(join(repoDir, ".env"))
      expect(stderr).toContain(cursorPath)
      expect(stderr).toContain("--src <number>")
    })
  })

  test("reports conflicts between explicit --src and env values", async () => {
    await withTempConfig(async (dir) => {
      const home = join(dir, "home")
      const cursorPath = join(home, ".cursor", "mcp.json")
      await mkdir(join(home, ".cursor"), { recursive: true })
      await writeFile(
        cursorPath,
        JSON.stringify({
          mcpServers: {
            telegram: {
              env: {
                TELEGRAM_API_ID: "222",
              },
            },
          },
        }),
        "utf8",
      )

      const result = Bun.spawnSync({
        cmd: ["bun", cliPath, "profile", "add", "--src", "1"],
        cwd: packageRoot,
        env: {
          ...process.env,
          HOME: home,
          TG_SAI_DIR: dir,
          TELEGRAM_API_ID: "111",
          TELEGRAM_API_HASH: "",
          TELEGRAM_SESSION_STRING: "",
          TELEGRAM_PASSWORD: "",
        },
        stdout: "pipe",
        stderr: "pipe",
      })

      const stderr = new TextDecoder().decode(result.stderr)
      expect(result.exitCode).toBe(1)
      expect(stderr).toContain(
        "Conflicting Telegram credential values for TELEGRAM_API_ID",
      )
      expect(stderr).toContain("CLI/env")
      expect(stderr).toContain(cursorPath)
    })
  })

  test("find reports discovered credential sources without adding", async () => {
    await withTempConfig(async (dir) => {
      const home = join(dir, "home")
      const cursorPath = join(home, ".cursor", "mcp.json")
      await mkdir(join(home, ".cursor"), { recursive: true })
      await writeFile(
        cursorPath,
        JSON.stringify({
          mcpServers: {
            telegram: {
              env: {
                TELEGRAM_API_ID: "987",
                TELEGRAM_API_HASH: "find-hash",
                TELEGRAM_SESSION_STRING: "find-session",
              },
            },
          },
        }),
        "utf8",
      )

      const result = Bun.spawnSync({
        cmd: ["bun", cliPath, "profile", "find"],
        cwd: packageRoot,
        env: {
          ...process.env,
          HOME: home,
          TG_SAI_DIR: dir,
          TELEGRAM_API_ID: "",
          TELEGRAM_API_HASH: "",
          TELEGRAM_SESSION_STRING: "",
          TELEGRAM_PASSWORD: "",
        },
        stdout: "pipe",
        stderr: "pipe",
      })

      const stdout = new TextDecoder().decode(result.stdout)
      expect(result.exitCode).toBe(0)
      expect(stdout).toContain("platform:")
      expect(stdout).toContain("home:")
      expect(stdout).toContain("checked:")
      expect(stdout).toContain("Cursor:")
      expect(stdout).toContain("candidates:")
      expect(stdout).toContain("1. Cursor:")
      expect(stdout).toContain("$.mcpServers.telegram.env")
      expect(stdout).toContain("[complete]")
      expect(stdout).toContain("tg-sai profile add")
      expect(stdout).not.toContain("tg-sai profile add --src <number>")
      expect(stdout).not.toContain("find-hash")
      expect(stdout).not.toContain("find-session")
    })
  })
})
