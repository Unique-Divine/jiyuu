import { describe, expect, test } from "bun:test"
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs"
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  bareHelpTarget,
  createProgram,
  getTelegramEnv,
  HOLD_EXIT_CODE,
  holdExitFor,
  parseDialogLimit,
  shortDuration,
  withPasswordRead,
} from "./main"
import type { TelegramEnv } from "./main"
import { installFakePass } from "./testdata"
import { addProfile, configDir } from "./profiles"
import { HoldAccountUnknownError, HoldError } from "./state/guard"
import { HoldStoreUnavailableError } from "./state/holds"

const NOW_MS = Date.UTC(2026, 10, 6, 12, 0, 0)

interface RunResult {
  exitCode: number
  stdout: string
  stderr: string
}

/** Runs the real CLI with a throwaway TG_SAI_DIR and no credentials. */
function runCli(args: readonly string[], dir: string): RunResult {
  const result = Bun.spawnSync({
    cmd: ["bun", "main.ts", ...args],
    cwd: import.meta.dir,
    env: {
      ...process.env,
      TG_SAI_DIR: dir,
      TG_SAI_REPO_DIR: join(dir, "repo"),
      HOME: join(dir, "home"),
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

function withDir<T>(fn: (dir: string) => T): T {
  const dir = mkdtempSync(join(tmpdir(), "tg-sai-main-"))
  try {
    return fn(dir)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

describe("holdExitFor", () => {
  test("a HoldError wrapped in error.cause exits 75 with the UTC time", () => {
    const untilMs = NOW_MS + 4 * 3_600_000 + 16 * 60_000
    const hold = new HoldError(untilMs, "invite", NOW_MS, "channels.invite")
    const wrapped = new Error("provisioning failed", {
      cause: new Error("addChatMembers failed", { cause: hold }),
    })
    const exit = holdExitFor(wrapped, NOW_MS)
    expect(exit?.exitCode).toBe(HOLD_EXIT_CODE)
    expect(exit?.message).toBe(
      "tg-sai: Telegram is limiting invite calls for this account; " +
        "nothing was sent. Resume after 2026-11-06T16:16:00Z (in 4h 16m).",
    )
    expect(exit?.message).not.toContain("\n")
  })

  test("a bare 420 TG_SAI_HOLD_<s> error uses its seconds", () => {
    const rpcLike = Object.assign(new Error("TG_SAI_HOLD_90"), {
      code: 420,
      text: "TG_SAI_HOLD_90",
    })
    expect(holdExitFor(rpcLike, NOW_MS)?.message).toContain(
      "Resume after 2026-11-06T12:01:30Z (in 1m 30s)",
    )
  })

  test("an unreadable state.db exits 75", () => {
    const exit = holdExitFor(
      new Error("wrapped", { cause: new HoldStoreUnavailableError("test") }),
      NOW_MS,
    )
    expect(exit?.exitCode).toBe(75)
    expect(exit?.message).toContain("state.db")
    expect(exit?.message).toContain("invite/create")
  })

  test("an unverified session account exits 75", () => {
    const exit = holdExitFor(
      new HoldAccountUnknownError("channels.inviteToChannel"),
      NOW_MS,
    )
    expect(exit?.exitCode).toBe(75)
    expect(exit?.message).toContain("nothing was sent")
    expect(exit?.message).not.toContain("\n")
  })

  test("other errors are not holds", () => {
    expect(holdExitFor(new Error("FLOOD_WAIT_30"), NOW_MS)).toBeNull()
    const other = Object.assign(new Error("TG_SAI_HOLD_5"), { code: 400 })
    expect(holdExitFor(other, NOW_MS)).toBeNull()
    expect(holdExitFor("TG_SAI_HOLD_5x", NOW_MS)).toBeNull()
    expect(holdExitFor(null, NOW_MS)).toBeNull()
  })

  test("shortDuration", () => {
    expect(shortDuration(40)).toBe("40s")
    expect(shortDuration(725)).toBe("12m 5s")
    expect(shortDuration(15_382)).toBe("4h 16m")
  })
})

describe("bare commands print help", () => {
  test("bareHelpTarget picks leaves with required input and no args", () => {
    const program = createProgram()
    expect(bareHelpTarget(program, ["audit"])?.name()).toBe("audit")
    expect(bareHelpTarget(program, ["audit", "--chat", "1"])).toBeNull()
    expect(bareHelpTarget(program, ["audit", "--bogus"])).toBeNull()
    expect(bareHelpTarget(program, ["health"])).toBeNull()
    expect(bareHelpTarget(program, ["ops"])).toBeNull()
    expect(bareHelpTarget(program, [])).toBeNull()
  })

  test("a bare leaf with required options prints help, exit 0", () => {
    withDir((dir) => {
      const result = runCli(["audit"], dir)
      expect(result.exitCode).toBe(0)
      expect(result.stdout).toContain("Usage: tg-sai audit")
      expect(result.stderr).toBe("")
    })
  })

  test("bare provision run and worker print help, exit 0", () => {
    withDir((dir) => {
      for (const args of [["chat", "provision", "run"], ["worker"]]) {
        const result = runCli(args, dir)
        expect(result.exitCode).toBe(0)
        expect(result.stdout).toContain(`Usage: tg-sai ${args.join(" ")}`)
      }
    })
  })

  test("partial args keep commander's error", () => {
    withDir((dir) => {
      const result = runCli(["audit", "--target-user", "1"], dir)
      expect(result.exitCode).toBe(1)
      expect(result.stderr).toContain("required option '--chat <id>'")
    })
  })

  test("a bare group prints help, exit 0, and records no run", () => {
    withDir((dir) => {
      const result = runCli(["ops"], dir)
      expect(result.exitCode).toBe(0)
      expect(result.stdout).toContain("Usage: tg-sai ops")
      expect(existsSync(join(dir, "telemetry.db"))).toBe(false)
    })
  })
})

describe("no JSONL run logs", () => {
  test("health --offline writes telemetry.db and nothing under logs/", () => {
    withDir((dir) => {
      const result = runCli(["health", "--offline"], dir)
      expect(result.exitCode).toBe(0)
      expect(existsSync(join(dir, "telemetry.db"))).toBe(true)
      const logs = join(dir, "logs")
      expect(existsSync(logs) ? readdirSync(logs) : []).toEqual([])
    })
  })
})

describe("ops sai-find --limit", () => {
  test("parseDialogLimit accepts positive integers", () => {
    expect(parseDialogLimit("500")).toBe(500)
    expect(parseDialogLimit("1")).toBe(1)
  })

  test("parseDialogLimit rejects values that are not positive integers", () => {
    for (const value of ["abc", "", "0", "-5", "2.5", "1e3", "Infinity"]) {
      expect(() => parseDialogLimit(value)).toThrow(
        "--limit must be a positive integer",
      )
    }
  })

  test("a non-numeric --limit fails before resolving credentials", () => {
    withDir((dir) => {
      const result = runCli(["ops", "sai-find", "--limit", "abc"], dir)
      expect(result.exitCode).toBe(1)
      expect(result.stderr).toContain("--limit must be a positive integer")
      expect(result.stderr).not.toContain("Missing Telegram credentials")
    })
  })
})

describe("getTelegramEnv", () => {
  /** Temp HOME holding a profile and a `~/.tg-sai/.env`; nothing real. */
  async function withSources<T>(
    profile: { name: string; complete: boolean; password: string },
    fn: (env: Record<string, string>) => Promise<T>,
  ): Promise<T> {
    const home = await mkdtemp(join(tmpdir(), "tg-sai-env-test-"))
    try {
      const env = { HOME: home, TG_SAI_REPO_DIR: join(home, "no-repo") }
      await addProfile(
        {
          name: profile.name,
          apiId: profile.complete ? 456 : null,
          apiHash: profile.complete ? "profile-hash" : "",
          sessionString: profile.complete ? "profile-session" : "",
          password: profile.password,
          handle: profile.complete ? "profile_handle" : null,
          userId: profile.complete ? 222 : null,
          displayName: "",
        },
        env,
      )
      await mkdir(configDir(env), { recursive: true })
      await writeFile(
        join(configDir(env), ".env"),
        [
          "TELEGRAM_API_ID=123",
          "TELEGRAM_API_HASH=dotenv-hash",
          "TELEGRAM_SESSION_STRING=dotenv-session",
        ].join("\n"),
        "utf8",
      )
      return await fn(env)
    } finally {
      await rm(home, { recursive: true, force: true })
    }
  }

  test("profile source carries the profile's password, name, and userId", async () => {
    await withSources(
      { name: "custody", complete: true, password: "custody-2fa" },
      async (env) => {
        expect(await getTelegramEnv(undefined, env)).toEqual({
          apiId: 456,
          apiHash: "profile-hash",
          session: "profile-session",
          password: { kind: "value", value: "custody-2fa" },
          userId: 222,
          profileName: "custody",
        })
      },
    )
  })

  test("env source takes no profile password or label", async () => {
    await withSources(
      { name: "custody", complete: true, password: "custody-2fa" },
      async (env) => {
        expect(
          await getTelegramEnv("custody", {
            ...env,
            TELEGRAM_API_ID: "999",
            TELEGRAM_API_HASH: "env-hash",
            TELEGRAM_SESSION_STRING: "env-session",
          }),
        ).toEqual({
          apiId: 999,
          apiHash: "env-hash",
          session: "env-session",
          password: null,
          userId: null,
          profileName: null,
        })
      },
    )
  })

  test("known-file fallback takes no draft password or label", async () => {
    await withSources(
      { name: "draft", complete: false, password: "draft-2fa" },
      async (env) => {
        expect(await getTelegramEnv(undefined, env)).toEqual({
          apiId: 123,
          apiHash: "dotenv-hash",
          session: "dotenv-session",
          password: null,
          userId: null,
          profileName: null,
        })
      },
    )
  })
})

describe("withPasswordRead", () => {
  const base: TelegramEnv = {
    apiId: 1,
    apiHash: "h",
    session: "s",
    password: null,
    userId: null,
    profileName: null,
  }

  test("reads a pass entry once and keeps other sources as they are", async () => {
    const fakePass = installFakePass()
    const store = join(fakePass.storeRoot, "store")
    const saved = { PATH: process.env.PATH, PASSWORD_STORE_DIR: process.env.PASSWORD_STORE_DIR }
    process.env.PATH = fakePass.path
    process.env.PASSWORD_STORE_DIR = store
    try {
      await mkdir(join(store, "tg-sai", "c"), { recursive: true })
      await writeFile(join(store, "tg-sai", "c", "telegram-2fa.gpg"), "fake-2fa\n")
      const read = await withPasswordRead({
        ...base,
        password: { kind: "pass", entry: "tg-sai/c/telegram-2fa" },
      })
      expect(read.password).toEqual({ kind: "value", value: "fake-2fa" })
      expect(await withPasswordRead(base)).toBe(base)
      const value = { ...base, password: { kind: "value" as const, value: "v" } }
      expect(await withPasswordRead(value)).toBe(value)
      await expect(
        withPasswordRead({ ...base, password: { kind: "pass", entry: "missing" } }),
      ).rejects.toThrow("pass show failed for entry missing")
    } finally {
      process.env.PATH = saved.PATH
      if (saved.PASSWORD_STORE_DIR === undefined) delete process.env.PASSWORD_STORE_DIR
      else process.env.PASSWORD_STORE_DIR = saved.PASSWORD_STORE_DIR
      await rm(join(fakePass.storeRoot, ".."), { recursive: true, force: true })
    }
  })
})
