import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { describe, expect, test } from "bun:test"

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

function runCli(args: string[], configDir: string): RunResult {
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
        cmd: ["bun", cliPath, "profile", "add"],
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
