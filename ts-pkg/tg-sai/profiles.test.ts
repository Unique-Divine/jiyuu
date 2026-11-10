import { describe, expect, test } from "bun:test"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import {
  addProfile,
  configDir,
  configPath,
  credentialsFromKnownFiles,
  credentialsFromText,
  discoveryPaths,
  discoveryReport,
  findTelegramCredentialBlocks,
  parseCredentialLine,
  parseCredentialValueAfterKey,
  platformKind,
  readConfig,
  redactedProfile,
  resolveTelegramCredentialSource,
  strictProfileCredentials,
} from "./profiles"
import type { ProfileAddInput } from "./profiles"

async function withTempHome<T>(fn: (home: string) => Promise<T>): Promise<T> {
  const home = await mkdtemp(join(tmpdir(), "tg-sai-home-test-"))
  try {
    return await fn(home)
  } finally {
    await rm(home, { recursive: true, force: true })
  }
}

async function writeJson(path: string, value: unknown): Promise<void> {
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`, "utf8")
}

describe("tg-sai profile discovery", () => {
  test("profile persistence rejects replacement of a known account before writing", async () => {
    await withTempHome(async (home) => {
      const env = { HOME: home }
      const original = { name: "custody", apiId: 456, apiHash: "custody-hash",
        sessionString: "custody-session", password: "", handle: "custody_handle",
        userId: 222, displayName: "Custody fixture" }
      await addProfile(original, env)
      const path = configPath(env)
      const before = await readFile(path, "utf8")
      await expect(addProfile({ ...original, userId: 111, sessionString: "personal-session" }, env))
        .rejects.toThrow("Cannot replace a saved profile's account identity")
      expect(await readFile(path, "utf8")).toBe(before)
    })
  })

  test("uses ~/.tg-sai as the default tool directory", async () => {
    await withTempHome(async (home) => {
      expect(configDir({ HOME: home })).toBe(join(home, ".tg-sai"))
    })
  })

  test("allows TG_SAI_DIR override", async () => {
    await withTempHome(async (home) => {
      expect(configDir({ HOME: home, TG_SAI_DIR: "/tmp/tg-sai" })).toBe(
        "/tmp/tg-sai",
      )
    })
  })

  test("migrates legacy record config to ordered profiles", async () => {
    await withTempHome(async (home) => {
      const dir = join(home, ".tg-sai")
      await mkdir(dir, { recursive: true })
      await writeFile(
        configPath({ HOME: home, TG_SAI_DIR: dir }),
        `${JSON.stringify(
          {
            activeProfile: "bob",
            profiles: {
              alice: {
                name: "alice",
                apiId: 1,
                apiHash: "alice-hash",
                sessionString: "alice-session",
                password: "",
                handle: "alice_handle",
                userId: 111,
                displayName: "Alice",
                createdAt: "2026-06-30T00:00:00.000Z",
                updatedAt: "2026-06-30T00:00:00.000Z",
              },
              bob: {
                name: "bob",
                apiId: 2,
                apiHash: "bob-hash",
                sessionString: "bob-session",
                password: "",
                handle: "bob_handle",
                userId: 222,
                displayName: "Bob",
                createdAt: "2026-06-30T00:00:00.000Z",
                updatedAt: "2026-06-30T00:00:00.000Z",
              },
            },
          },
          null,
          2,
        )}\n`,
        "utf8",
      )

      const config = await readConfig({ HOME: home, TG_SAI_DIR: dir })

      expect(config.profiles.map((profile) => profile.name)).toEqual([
        "bob",
        "alice",
      ])
      expect(config.profiles[0]?.handle).toBe("bob_handle")
    })
  })

  test("categorizes runtime platforms", () => {
    expect(platformKind("darwin")).toBe("macos")
    expect(platformKind("linux")).toBe("linux")
    expect(platformKind("win32")).toBe("windows")
    expect(platformKind("freebsd")).toBe("unknown")
  })

  test("uses macOS-specific discovery paths under HOME", async () => {
    await withTempHome(async (home) => {
      const paths = discoveryPaths({ HOME: home }, "macos")

      expect(paths.map((path) => path.name)).toEqual([
        "tg-sai repo .env",
        "tg-sai user .env",
        "Claude Desktop",
        "Cursor",
        "Claude Code",
      ])
      expect(paths[2]?.expandedPath).toBe(
        join(
          home,
          "Library",
          "Application Support",
          "Claude",
          "claude_desktop_config.json",
        ),
      )
    })
  })

  test("does not check the macOS Claude Desktop path on Linux", async () => {
    await withTempHome(async (home) => {
      const paths = discoveryPaths({ HOME: home }, "linux")

      expect(paths.map((path) => path.name)).toEqual([
        "tg-sai repo .env",
        "tg-sai user .env",
        "Cursor",
        "Claude Code",
      ])
      expect(paths.map((path) => path.expandedPath).join("\n")).not.toContain(
        "Library/Application Support/Claude",
      )
    })
  })

  test("uses Windows-specific discovery paths under HOME", async () => {
    await withTempHome(async (home) => {
      const paths = discoveryPaths({ HOME: home }, "windows")

      expect(paths.map((path) => path.name)).toEqual([
        "tg-sai repo .env",
        "tg-sai user .env",
        "Cursor",
        "Claude Code",
      ])
      expect(paths[2]?.expandedPath).toBe(
        join(home, "AppData", "Roaming", "Cursor", "User", "mcp.json"),
      )
    })
  })

  test("parses credential values after JSON and shell separators", () => {
    expect(parseCredentialValueAfterKey('": "77777777",')).toBe("77777777")
    expect(parseCredentialValueAfterKey('="dddd"')).toBe("dddd")
    expect(parseCredentialValueAfterKey("='bbbb'")).toBe("bbbb")
    expect(parseCredentialValueAfterKey("=cccc # comment")).toBe("cccc")
  })

  test("parses individual JSON-like and shell-like credential lines", () => {
    expect(
      parseCredentialLine('... "TELEGRAM_API_ID": "77777777", ...'),
    ).toEqual({ TELEGRAM_API_ID: "77777777" })
    expect(parseCredentialLine("export TELEGRAM_API_HASH='bbbb'")).toEqual({
      TELEGRAM_API_HASH: "bbbb",
    })
    expect(parseCredentialLine('TELEGRAM_API_HASH="dddd"')).toEqual({
      TELEGRAM_API_HASH: "dddd",
    })
    expect(parseCredentialLine("TELEGRAM_SESSION_STRING=session")).toEqual({
      TELEGRAM_SESSION_STRING: "session",
    })
    expect(parseCredentialLine("TELEGRAM_PASSWORD='secret'")).toEqual({
      TELEGRAM_PASSWORD: "secret",
    })
  })

  test("builds credentials from line scanning mixed content", () => {
    const credentials = credentialsFromText(`
      broken json around here
      "TELEGRAM_API_ID": "77777777",
      export TELEGRAM_API_HASH='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
      TELEGRAM_SESSION_STRING=session-value
    `)

    expect(credentials).toEqual({
      apiId: 77777777,
      apiHash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      sessionString: "session-value",
    })
  })

  test("finds nested Telegram credential blocks", () => {
    const blocks = findTelegramCredentialBlocks({
      mcpServers: {
        telegram: {
          env: {
            TELEGRAM_API_ID: "123",
            TELEGRAM_API_HASH: "hash",
            TELEGRAM_SESSION_STRING: "session",
          },
        },
      },
    })

    expect(blocks).toHaveLength(1)
    expect(blocks[0]?.objectPath).toBe("$.mcpServers.telegram.env")
    expect(blocks[0]?.credentials).toEqual({
      apiId: 123,
      apiHash: "hash",
      sessionString: "session",
    })
  })

  test("uses neighboring fields from a JSON credential candidate object", () => {
    const blocks = findTelegramCredentialBlocks({
      nested: {
        maybeTelegram: {
          TELEGRAM_API_ID: 999,
          TELEGRAM_API_HASH: "neighbor-hash",
          TELEGRAM_SESSION_STRING: "neighbor-session",
          OTHER_VALUE: "ignored",
        },
      },
    })

    expect(blocks).toHaveLength(1)
    expect(blocks[0]?.objectPath).toBe("$.nested.maybeTelegram")
    expect(blocks[0]?.credentials).toEqual({
      apiId: 999,
      apiHash: "neighbor-hash",
      sessionString: "neighbor-session",
    })
  })

  test("reports incomplete JSON credential candidate objects", () => {
    const blocks = findTelegramCredentialBlocks({
      nested: {
        incomplete: {
          TELEGRAM_API_ID: 999,
          TELEGRAM_API_HASH: "neighbor-hash",
        },
      },
    })

    expect(blocks).toHaveLength(1)
    expect(blocks[0]?.objectPath).toBe("$.nested.incomplete")
    expect(blocks[0]?.values).toEqual({
      TELEGRAM_API_ID: "999",
      TELEGRAM_API_HASH: "neighbor-hash",
    })
    expect(blocks[0]?.credentials).toBeNull()
    expect(blocks[0]?.missingKeys).toEqual(["TELEGRAM_SESSION_STRING"])
  })

  test("builds a macOS discovery report from fake HOME", async () => {
    await withTempHome(async (home) => {
      await writeJson(
        join(
          home,
          "Library",
          "Application Support",
          "Claude",
          "claude_desktop_config.json",
        ),
        {
          mcpServers: {
            telegram: {
              env: {
                TELEGRAM_API_ID: "321",
                TELEGRAM_API_HASH: "claude-hash",
                TELEGRAM_SESSION_STRING: "claude-session",
              },
            },
          },
        },
      )

      const report = await discoveryReport({ HOME: home }, "macos")

      expect(report.platform).toBe("macos")
      expect(report.checkedPaths).toHaveLength(5)
      expect(report.foundFiles).toHaveLength(1)
      expect(report.candidates).toHaveLength(1)
      expect(report.candidates[0]?.name).toBe("Claude Desktop")
      expect(report.candidates[0]?.credentials).not.toBeNull()
      expect(report.candidates[0]?.credentials?.sessionString).toBe(
        "claude-session",
      )
    })
  })

  test("auto-uses exactly one discovered candidate", async () => {
    await withTempHome(async (home) => {
      await writeJson(join(home, ".cursor", "mcp.json"), {
        mcpServers: {
          telegram: {
            env: {
              TELEGRAM_API_ID: "456",
              TELEGRAM_API_HASH: "cursor-hash",
              TELEGRAM_SESSION_STRING: "cursor-session",
            },
          },
        },
      })

      const credentials = await credentialsFromKnownFiles({ HOME: home })

      expect(credentials).toEqual({
        apiId: 456,
        apiHash: "cursor-hash",
        sessionString: "cursor-session",
      })
    })
  })

  test("discovers credentials and password from tg-sai env files", async () => {
    await withTempHome(async (home) => {
      const repoDir = join(home, "repo-tg-sai")
      const toolDir = join(home, ".tg-sai")
      await mkdir(repoDir, { recursive: true })
      await mkdir(toolDir, { recursive: true })
      await writeFile(
        join(repoDir, ".env"),
        [
          "TELEGRAM_API_ID=123",
          "TELEGRAM_API_HASH=repo-hash",
          "TELEGRAM_SESSION_STRING=repo-session",
        ].join("\n"),
        "utf8",
      )
      await writeFile(
        join(toolDir, ".env"),
        "TELEGRAM_PASSWORD='profile-secret'\n",
        "utf8",
      )

      const report = await discoveryReport(
        {
          HOME: home,
          TG_SAI_DIR: toolDir,
          TG_SAI_REPO_DIR: repoDir,
        },
        "linux",
      )

      expect(report.foundFiles).toEqual([
        join(repoDir, ".env"),
        join(toolDir, ".env"),
      ])
      expect(report.candidates[0]?.name).toBe("tg-sai repo .env")
      expect(report.candidates[0]?.credentials).toEqual({
        apiId: 123,
        apiHash: "repo-hash",
        sessionString: "repo-session",
      })
      expect(report.candidates[1]?.values).toEqual({
        TELEGRAM_PASSWORD: "profile-secret",
      })
    })
  })

  test("reports partial discovered candidates without auto-using them", async () => {
    await withTempHome(async (home) => {
      await writeJson(join(home, ".cursor", "mcp.json"), {
        mcpServers: {
          telegram: {
            env: {
              TELEGRAM_API_ID: "456",
              TELEGRAM_API_HASH: "cursor-hash",
            },
          },
        },
      })

      const report = await discoveryReport({ HOME: home }, "linux")
      const credentials = await credentialsFromKnownFiles({ HOME: home })

      expect(report.candidates).toHaveLength(1)
      expect(report.candidates[0]?.credentials).toBeNull()
      expect(report.candidates[0]?.missingKeys).toEqual([
        "TELEGRAM_SESSION_STRING",
      ])
      expect(credentials).toBeNull()
    })
  })

  test("falls back to line scanning malformed JSON discovery files", async () => {
    await withTempHome(async (home) => {
      const path = join(home, ".cursor", "mcp.json")
      await mkdir(dirname(path), { recursive: true })
      await writeFile(
        path,
        `
        {
          "mcpServers": {
            "telegram": {
              "env": {
                "TELEGRAM_API_ID": "654",
                "TELEGRAM_API_HASH": "line-hash",
                "TELEGRAM_SESSION_STRING": "line-session",
              }
        `,
        "utf8",
      )

      const report = await discoveryReport({ HOME: home }, "linux")

      expect(report.candidates).toHaveLength(1)
      expect(report.candidates[0]?.objectPath).toBe("$.lineScan")
      expect(report.candidates[0]?.credentials).toEqual({
        apiId: 654,
        apiHash: "line-hash",
        sessionString: "line-session",
      })
    })
  })
})

/** Temp-HOME env for the resolver: no repo `.env`, no TELEGRAM_* values. */
function resolverEnv(home: string): Record<string, string> {
  return { HOME: home, TG_SAI_REPO_DIR: join(home, "no-repo") }
}

/** Profile fixture; override fields per test. */
function profileInput(
  overrides: Partial<ProfileAddInput> = {},
): ProfileAddInput {
  return {
    name: "custody",
    apiId: 456,
    apiHash: "custody-hash",
    sessionString: "custody-session",
    password: "",
    handle: "custody_handle",
    userId: 222,
    displayName: "Custody fixture",
    ...overrides,
  }
}

/** Blank onboarding draft: no credentials and no account identity. */
const blankDraft = profileInput({
  name: "draft",
  apiId: null,
  apiHash: "",
  sessionString: "",
  handle: null,
  userId: null,
  displayName: "",
})

/** Writes `~/.tg-sai/.env` under the temp HOME. */
async function writeUserDotEnv(
  env: Record<string, string>,
  lines: readonly string[],
): Promise<void> {
  await mkdir(configDir(env), { recursive: true })
  await writeFile(join(configDir(env), ".env"), lines.join("\n"), "utf8")
}

const dotEnvLines = [
  "TELEGRAM_API_ID=123",
  "TELEGRAM_API_HASH=dotenv-hash",
  "TELEGRAM_SESSION_STRING=dotenv-session",
] as const

const dotEnvCredentials = {
  apiId: 123,
  apiHash: "dotenv-hash",
  sessionString: "dotenv-session",
}

describe("resolveTelegramCredentialSource", () => {
  test("reports env, profile, and known-file credentials", async () => {
    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      await addProfile(profileInput({ password: "custody-2fa" }), env)
      expect(await resolveTelegramCredentialSource("custody", env)).toEqual({
        credentials: {
          apiId: 456,
          apiHash: "custody-hash",
          sessionString: "custody-session",
        },
        source: "profile",
        password: { kind: "value", value: "custody-2fa" },
        profileName: "custody",
        userId: 222,
      })

      // TELEGRAM_* wins over the selected profile and takes none of its
      // password, name, or userId.
      expect(
        await resolveTelegramCredentialSource("custody", {
          ...env,
          TELEGRAM_API_ID: "999",
          TELEGRAM_API_HASH: "env-hash",
          TELEGRAM_SESSION_STRING: "env-session",
        }),
      ).toEqual({
        credentials: {
          apiId: 999,
          apiHash: "env-hash",
          sessionString: "env-session",
        },
        source: "env",
        password: null,
        profileName: null,
        userId: null,
      })
    })

    await withTempHome(async (home) => {
      const file = join(home, "creds.env")
      await writeFile(
        file,
        [...dotEnvLines, "TELEGRAM_PASSWORD=file-2fa"].join("\n"),
        "utf8",
      )
      expect(
        await resolveTelegramCredentialSource(undefined, {
          HOME: home,
          TG_SAI_DISCOVERY_FILE: file,
        }),
      ).toEqual({
        credentials: dotEnvCredentials,
        source: "known_file",
        password: { kind: "value", value: "file-2fa" },
        profileName: null,
        userId: null,
      })
    })
  })

  test("a complete profile wins over a complete ~/.tg-sai/.env", async () => {
    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      await addProfile(profileInput(), env)
      await writeUserDotEnv(env, dotEnvLines)
      for (const selector of [undefined, "custody"]) {
        const found = await resolveTelegramCredentialSource(selector, env)
        expect(found.source).toBe("profile")
        expect(found.credentials.sessionString).toBe("custody-session")
      }
    })
  })

  test("a blank active draft falls back to ~/.tg-sai/.env", async () => {
    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      await addProfile(blankDraft, env)
      await writeUserDotEnv(env, dotEnvLines)
      expect(await resolveTelegramCredentialSource(undefined, env)).toEqual({
        credentials: dotEnvCredentials,
        source: "known_file",
        password: null,
        profileName: null,
        userId: null,
      })
    })
  })

  test("an explicit --profile never falls back to another account", async () => {
    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      await addProfile(blankDraft, env)
      await writeUserDotEnv(env, dotEnvLines)
      await expect(
        resolveTelegramCredentialSource("draft", env),
      ).rejects.toThrow("Profile draft is incomplete")
    })
  })

  test("an explicit --profile matching no profile never falls back", async () => {
    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      await addProfile(profileInput(), env)
      await writeUserDotEnv(env, dotEnvLines)
      // A misspelled name and an out-of-range index.
      for (const selector of ["custdy", "7"]) {
        await expect(
          resolveTelegramCredentialSource(selector, env),
        ).rejects.toThrow(`Unknown profile: ${selector}`)
      }
    })
  })

  test("an incomplete profile with an identity never falls back", async () => {
    for (const identity of [
      { userId: 333, handle: null },
      { userId: null, handle: "named_account" },
    ]) {
      await withTempHome(async (home) => {
        const env = resolverEnv(home)
        await addProfile({ ...blankDraft, ...identity }, env)
        await writeUserDotEnv(env, dotEnvLines)
        await expect(
          resolveTelegramCredentialSource(undefined, env),
        ).rejects.toThrow("Profile draft is incomplete")
      })
    }
  })

  test("names the blank draft when no other source has credentials", async () => {
    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      await addProfile(blankDraft, env)
      await expect(
        resolveTelegramCredentialSource(undefined, env),
      ).rejects.toThrow("Profile draft is incomplete")
    })
  })

  test("passwords follow the session source; blanks are null", async () => {
    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      // A draft that stores a password must not lend it to the .env session.
      await addProfile({ ...blankDraft, password: "draft-2fa" }, env)
      await writeUserDotEnv(env, [...dotEnvLines, "TELEGRAM_PASSWORD="])
      const fallback = await resolveTelegramCredentialSource(undefined, env)
      expect(fallback.source).toBe("known_file")
      expect(fallback.password).toBeNull()

      // TELEGRAM_PASSWORD still overrides for one command; blank is null.
      expect(
        (
          await resolveTelegramCredentialSource(undefined, {
            ...env,
            TELEGRAM_PASSWORD: "cli-2fa",
          })
        ).password,
      ).toEqual({ kind: "value", value: "cli-2fa" })
      expect(
        (
          await resolveTelegramCredentialSource(undefined, {
            ...env,
            TELEGRAM_PASSWORD: "",
          })
        ).password,
      ).toBeNull()
    })

    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      await addProfile(profileInput({ password: "" }), env)
      expect(
        (await resolveTelegramCredentialSource("custody", env)).password,
      ).toBeNull()
    })
  })
})

describe("2FA password in pass", () => {
  test("a profile keeps only the pass entry and resolution never runs pass", async () => {
    await withTempHome(async (home) => {
      // No pass on PATH: resolving must not read the entry.
      const env = { ...resolverEnv(home), PATH: "" }
      await addProfile(profileInput({ password: "legacy-2fa" }), env)
      await addProfile(
        profileInput({ passwordPassEntry: "tg-sai/custody/telegram-2fa" }),
        env,
      )
      const raw = await readFile(configPath(env), "utf8")
      expect(raw).not.toContain("legacy-2fa")
      expect(JSON.parse(raw).profiles[0]).not.toHaveProperty("password")

      const resolved = await resolveTelegramCredentialSource("custody", env)
      expect(resolved.password).toEqual({
        kind: "pass",
        entry: "tg-sai/custody/telegram-2fa",
      })
      // TELEGRAM_PASSWORD still overrides for one command.
      expect(
        (
          await resolveTelegramCredentialSource("custody", {
            ...env,
            TELEGRAM_PASSWORD: "cli-2fa",
          })
        ).password,
      ).toEqual({ kind: "value", value: "cli-2fa" })

      const profile = (await readConfig(env)).profiles[0]!
      expect(redactedProfile(profile)).toMatchObject({
        password: "",
        passwordPassEntry: "tg-sai/custody/telegram-2fa",
        passwordStorage: "pass",
      })
    })
  })

  test("legacy plain-text profiles still resolve and report their storage", async () => {
    await withTempHome(async (home) => {
      const env = resolverEnv(home)
      await addProfile(profileInput({ password: "legacy-2fa" }), env)
      const profile = (await readConfig(env)).profiles[0]!
      expect(redactedProfile(profile)).toMatchObject({
        password: "[redacted]",
        passwordPassEntry: null,
        passwordStorage: "plain_text",
      })
      expect(
        (await resolveTelegramCredentialSource("custody", env)).password,
      ).toEqual({ kind: "value", value: "legacy-2fa" })
    })
  })

  test("custody credentials ignore the password reference and need no pass", async () => {
    await withTempHome(async (home) => {
      const env = { ...resolverEnv(home), PATH: "" }
      await addProfile(
        profileInput({ passwordPassEntry: "tg-sai/custody/telegram-2fa" }),
        env,
      )
      expect(await strictProfileCredentials("custody", 222, env)).toEqual({
        apiId: 456,
        apiHash: "custody-hash",
        sessionString: "custody-session",
      })
    })
  })
})
