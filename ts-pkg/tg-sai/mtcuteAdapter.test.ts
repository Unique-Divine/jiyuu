import {
  existsSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { afterEach, describe, expect, test } from "bun:test"

import {
  createMtcuteRuntimeClient,
  defaultSessionStoragePath,
  resolveSessionStorage,
  SESSION_CACHE_DIR,
} from "./mtcuteAdapter"

const dirs: string[] = []

function tempToolDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "tg-sai-session-"))
  dirs.push(dir)
  return join(dir, "tool")
}

afterEach(() => {
  for (const dir of dirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true })
  }
})

describe("session storage", () => {
  test("defaults into the tool directory, keyed by session", () => {
    const toolDir = tempToolDir()
    const env = { TG_SAI_DIR: toolDir }
    const a = defaultSessionStoragePath("session-a", env)
    expect(dirname(a)).toBe(join(toolDir, SESSION_CACHE_DIR))
    expect(a).toMatch(/[0-9a-f]{32}\.session$/)
    expect(defaultSessionStoragePath("session-a", env)).toBe(a)
    expect(defaultSessionStoragePath("session-b", env)).not.toBe(a)
  })

  test("never puts the session string in the file name", () => {
    const toolDir = tempToolDir()
    const session = "1BVtsOKABu0-secret-session-string"
    const path = defaultSessionStoragePath(session, { TG_SAI_DIR: toolDir })
    expect(path.includes(session)).toBe(false)
  })

  test("creates the default file with private modes", () => {
    const toolDir = tempToolDir()
    const env = { TG_SAI_DIR: toolDir }
    const path = resolveSessionStorage(undefined, "session-a", env)
    expect(path).toBe(defaultSessionStoragePath("session-a", env))
    expect(statSync(dirname(path)).mode & 0o777).toBe(0o700)
    expect(statSync(path).mode & 0o777).toBe(0o600)
  })

  test("keeps an explicit storage path untouched", () => {
    const toolDir = tempToolDir()
    const explicit = join(toolDir, "custom.session")
    const env = { TG_SAI_DIR: toolDir }
    expect(resolveSessionStorage(explicit, "session-a", env)).toBe(explicit)
    expect(existsSync(join(toolDir, SESSION_CACHE_DIR))).toBe(false)
  })

  test("a runtime client without storage writes no file", () => {
    const before = readdirSync(process.cwd())
    const client = createMtcuteRuntimeClient({
      apiId: 1,
      apiHash: "0".repeat(32),
    })
    expect(client).toBeDefined()
    expect(readdirSync(process.cwd())).toEqual(before)
  })
})
