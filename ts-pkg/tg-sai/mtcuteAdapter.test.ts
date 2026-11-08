import { mkdtempSync, rmSync, statSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { afterEach, describe, expect, test } from "bun:test"

import {
  DEFAULT_SESSION_FILE,
  defaultSessionStoragePath,
  resolveSessionStorage,
} from "./mtcuteAdapter"

const dirs: string[] = []

function tempToolDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "tg-sai-session-"))
  dirs.push(dir)
  return join(dir, "tool")
}

afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true })
})

describe("session storage", () => {
  test("defaults into the tool directory, not the working directory", () => {
    const toolDir = tempToolDir()
    const env = { TG_SAI_DIR: toolDir }
    expect(defaultSessionStoragePath(env)).toBe(
      join(toolDir, DEFAULT_SESSION_FILE),
    )
  })

  test("creates the default file with private modes", () => {
    const toolDir = tempToolDir()
    const path = resolveSessionStorage(undefined, { TG_SAI_DIR: toolDir })
    expect(path).toBe(join(toolDir, DEFAULT_SESSION_FILE))
    expect(statSync(toolDir).mode & 0o777).toBe(0o700)
    expect(statSync(path).mode & 0o777).toBe(0o600)
  })

  test("keeps an explicit storage path untouched", () => {
    const toolDir = tempToolDir()
    const explicit = join(toolDir, "custom.session")
    expect(resolveSessionStorage(explicit, { TG_SAI_DIR: toolDir })).toBe(
      explicit,
    )
  })
})
