import { describe, expect, test } from "bun:test"
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  bareHelpTarget,
  createProgram,
  HOLD_EXIT_CODE,
  holdExitFor,
  shortDuration,
} from "./main"
import { HoldError } from "./state/guard"
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
