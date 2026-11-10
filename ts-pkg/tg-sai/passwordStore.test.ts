import { afterAll, describe, expect, test } from "bun:test"
import { mkdtemp, readFile, rm } from "node:fs/promises"
import { dirname, join } from "node:path"
import {
  defaultPasswordPassEntry,
  insertPassPassword,
  passEntryExists,
  passwordFromStdinText,
  readPassPassword,
  readTelegramPassword,
  validatePassEntry,
} from "./passwordStore"
import { installFakePass } from "./testdata"

const fakePass = installFakePass()
afterAll(async () => {
  await rm(dirname(fakePass.binDir), { recursive: true, force: true })
})

async function passEnv() {
  const store = await mkdtemp(join(fakePass.storeRoot, "store-"))
  return { PATH: fakePass.path, PASSWORD_STORE_DIR: store, HOME: store }
}

describe("passwordStore", () => {
  test("default entry is per profile", () => {
    expect(defaultPasswordPassEntry("sai-custody")).toBe(
      "tg-sai/sai-custody/telegram-2fa",
    )
  })

  test.each(["", " a", "-rf", "/abs", "a/../b", "a//b", "a/", "a\nb"])(
    "rejects entry name %j",
    (entry) => {
      expect(() => validatePassEntry(entry)).toThrow("Invalid pass entry name")
    },
  )

  test("accepts nested entry names", () => {
    expect(validatePassEntry("tg-sai/alice/telegram-2fa")).toBe(
      "tg-sai/alice/telegram-2fa",
    )
  })

  test("stdin password is the first line and must be non-empty", () => {
    expect(passwordFromStdinText("fake-2fa\n")).toBe("fake-2fa")
    expect(passwordFromStdinText("fake-2fa\r\nignored\n")).toBe("fake-2fa")
    expect(() => passwordFromStdinText("\n")).toThrow("must not be empty")
  })

  test("inserts, finds, and reads back an entry", async () => {
    const env = await passEnv()
    expect(await passEntryExists("tg-sai/a/telegram-2fa", env)).toBe(false)
    await insertPassPassword("tg-sai/a/telegram-2fa", "fake-2fa-one", env)
    expect(await passEntryExists("tg-sai/a/telegram-2fa", env)).toBe(true)
    expect(await readPassPassword("tg-sai/a/telegram-2fa", env)).toBe(
      "fake-2fa-one",
    )
    expect(
      await readTelegramPassword(
        { kind: "pass", entry: "tg-sai/a/telegram-2fa" },
        env,
      ),
    ).toBe("fake-2fa-one")
  })

  test("reads only the first line of an entry", async () => {
    const env = await passEnv()
    await insertPassPassword("x", "fake-first", env)
    await Bun.write(
      join(env.PASSWORD_STORE_DIR, "x.gpg"),
      "fake-first\nurl: example\n",
    )
    expect(await readPassPassword("x", env)).toBe("fake-first")
  })

  test("a failed show names the entry, not pass output", async () => {
    const env = await passEnv()
    await expect(readPassPassword("missing/entry", env)).rejects.toThrow(
      "pass show failed for entry missing/entry",
    )
  })

  test("an in-memory value never runs pass", async () => {
    const env = await passEnv()
    expect(
      await readTelegramPassword(
        { kind: "value", value: "fake-env-2fa" },
        { ...env, PATH: "" },
      ),
    ).toBe("fake-env-2fa")
    expect(await Bun.file(join(env.PASSWORD_STORE_DIR, ".calls")).exists()).toBe(
      false,
    )
  })

  test("reports a missing pass binary", async () => {
    const env = await passEnv()
    await expect(
      readPassPassword("x", { ...env, PATH: "/nonexistent" }),
    ).rejects.toThrow("pass is not installed")
  })

  test("insert sends the password on stdin, not argv", async () => {
    const env = await passEnv()
    await insertPassPassword("argv-check", "fake-2fa-argv", env)
    const calls = await readFile(join(env.PASSWORD_STORE_DIR, ".calls"), "utf8")
    expect(calls).toBe("insert argv-check\n")
    expect(calls).not.toContain("fake-2fa-argv")
  })
})
