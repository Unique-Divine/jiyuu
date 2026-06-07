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
      expect(addAlice.stdout).toContain("profile_active: alice")
      expect(addAlice.stdout).toContain("user_id: 111")
      expect(addAlice.stdout).toContain("telegram_password: stored")
      expect(addAlice.stdout).toContain(
        "ownership_transfer: profile password or TELEGRAM_PASSWORD enables transfer",
      )

      const listOne = runCli(["profile", "list"], dir)
      expect(listOne.exitCode).toBe(0)
      expect(listOne.stdout).toContain("* alice")
      expect(listOne.stdout).toContain("alice_handle")

      const addBob = runCli(addBobArgs, dir)
      expect(addBob.exitCode).toBe(0)
      expect(addBob.stdout).toContain("profile_saved: bob")

      const show = runCli(["profile", "show", "alice"], dir)
      expect(show.exitCode).toBe(0)
      expect(show.stdout).toContain('"name": "alice"')
      expect(show.stdout).toContain('"apiHash": "[redacted]"')
      expect(show.stdout).toContain('"password": "[redacted]"')
      expect(show.stdout).not.toContain("session-alice")
      expect(show.stdout).not.toContain("password-alice")

      const useBob = runCli(["profile", "use", "bob"], dir)
      expect(useBob.exitCode).toBe(0)
      expect(useBob.stdout).toContain("active_profile: bob")

      const listTwo = runCli(["profile", "list"], dir)
      expect(listTwo.stdout).toContain("  alice")
      expect(listTwo.stdout).toContain("* bob")

      const removeBob = runCli(["profile", "remove", "bob"], dir)
      expect(removeBob.exitCode).toBe(0)
      expect(removeBob.stdout).toContain("removed_profile: bob")

      const listAfterRemove = runCli(["profile", "list"], dir)
      expect(listAfterRemove.stdout).toContain("* alice")
      expect(listAfterRemove.stdout).not.toContain("bob_handle")
    })
  })

  test("adds a draft profile with blank values", async () => {
    await withTempConfig(async (dir) => {
      const result = runCli(["profile", "add", "--name", "draft"], dir)

      expect(result.exitCode).toBe(0)
      expect(result.stdout).toContain("profile_active: draft")
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
        },
        stdout: "pipe",
        stderr: "pipe",
      })

      const stdout = new TextDecoder().decode(result.stdout)
      expect(result.exitCode).toBe(0)
      expect(stdout).toContain("profile_active: discovered")
      expect(stdout).toContain("telegram_password: missing")

      const show = runCli(["profile", "show", "discovered"], dir)
      expect(show.stdout).toContain('"apiId": 789')
      expect(show.stdout).not.toContain("discovered-session")
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
      expect(stdout).toContain(
        "tg-sai profile add --name <name> --src <number>",
      )
      expect(stdout).not.toContain("find-hash")
      expect(stdout).not.toContain("find-session")
    })
  })
})
