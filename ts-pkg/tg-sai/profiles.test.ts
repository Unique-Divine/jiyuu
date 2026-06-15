import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { describe, expect, test } from "bun:test"
import {
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
} from "./profiles"

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
