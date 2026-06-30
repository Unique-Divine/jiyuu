import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { describe, expect, test } from "bun:test"

import { runHealthCheck } from "./health"
import type { HealthClientFactory } from "./health"

async function withTempConfig<T>(fn: (dir: string) => Promise<T>): Promise<T> {
  const dir = await mkdtemp(join(tmpdir(), "tg-sai-health-test-"))
  try {
    return await fn(dir)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
}

function failingClientFactory(): HealthClientFactory {
  return async () => {
    throw new Error("client should not be created")
  }
}

const completeClientFactory: HealthClientFactory = async () => ({
  async getMe() {
    return {
      id: 7930936303,
      username: "corinnebernett",
      displayName: "Corinne Bernett",
      isBot: false,
    }
  },
  async disconnect() {},
})

const connectionFailureClientFactory: HealthClientFactory = async () => ({
  async getMe() {
    throw new Error("AUTH_KEY_UNREGISTERED")
  },
  async disconnect() {},
})

async function writeProfiles(
  dir: string,
  profile: {
    name: string
    apiId: number | null
    apiHash: string
    sessionString: string
    password?: string
  },
): Promise<void> {
  await writeFile(
    join(dir, "profiles.json"),
    `${JSON.stringify(
      {
        activeProfile: profile.name,
        profiles: {
          [profile.name]: {
            ...profile,
            handle: null,
            userId: null,
            displayName: "",
            password: profile.password ?? "",
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
}

describe("tg-sai health", () => {
  test("reports missing active profile and creates local directories", async () => {
    await withTempConfig(async (dir) => {
      const result = await runHealthCheck({
        env: {
          TG_SAI_DIR: dir,
        },
        createClient: failingClientFactory(),
      })

      expect(result.exitCode).toBe(1)
      expect(result.lines).toContain(`tool_dir: ${dir}`)
      expect(result.lines).toContain(`logs_dir: ${join(dir, "logs")}`)
      expect(result.lines).toContain("❌😭 active_profile: missing")
      expect(result.lines).toContain(
        "❌😭 telegram_connection: skipped because no active profile is configured",
      )
      expect(result.lines).toContain("suggested_fixes:")
      expect(result.lines).toContain("  tg-sai profile find")
      expect(result.lines).toContain(
        "  tg-sai profile add --name <name> --src <number>",
      )
      expect(result.lines).toContain("next: tg-sai profile find")

      await writeFile(join(dir, "logs", "write-check.txt"), "ok", "utf8")
      expect(await readFile(join(dir, "logs", "write-check.txt"), "utf8")).toBe(
        "ok",
      )
    })
  })

  test("reports incomplete active profile without connecting", async () => {
    await withTempConfig(async (dir) => {
      await writeProfiles(dir, {
        name: "corinne",
        apiId: 123,
        apiHash: "hash",
        sessionString: "",
      })

      const result = await runHealthCheck({
        env: {
          TG_SAI_DIR: dir,
        },
        createClient: failingClientFactory(),
      })

      expect(result.exitCode).toBe(1)
      expect(result.lines).toContain("active_profile: corinne")
      expect(result.lines).toContain("❌😭 profile_status: incomplete")
      expect(result.lines).toContain("❌😭 missing_fields: sessionString")
      expect(result.lines).toContain(
        "❌😭 telegram_connection: skipped because active profile is incomplete",
      )
      expect(result.lines).toContain("suggested_fixes:")
      expect(result.lines).toContain(
        "  Fill missing profile field(s): sessionString",
      )
      expect(result.lines).toContain("  tg-sai profile show corinne")
      expect(result.lines).toContain(
        "next: tg-sai profile add --name corinne --src <number>",
      )
    })
  })

  test("reports complete profile, password, and latest log files", async () => {
    await withTempConfig(async (dir) => {
      await writeProfiles(dir, {
        name: "corinne",
        apiId: 123,
        apiHash: "hash",
        sessionString: "session",
        password: "profile-password",
      })
      await mkdir(join(dir, "logs"), { recursive: true })
      await writeFile(
        join(dir, "logs", "sai-find-00.jsonl"),
        '{"chatId":1}\n{"chatId":2}\n',
        "utf8",
      )
      await writeFile(
        join(dir, "logs", "sai-bootstrap-02.jsonl"),
        '{"type":"run_complete"}\n',
        "utf8",
      )

      const result = await runHealthCheck({
        env: {
          TG_SAI_DIR: dir,
        },
        createClient: completeClientFactory,
      })

      expect(result.exitCode).toBe(0)
      expect(result.lines).toContain("✅🔥 profile_status: complete")
      expect(result.lines).toContain("✅🔥 telegram_connection: ok")
      expect(result.lines).toContain(
        'session_user: 7930936303 corinnebernett "Corinne Bernett"',
      )
      expect(result.lines).toContain("password_status: present")
      expect(result.lines).not.toContain("suggested_fixes:")
      expect(result.lines).toContain(
        `latest_sai_find: ${join(dir, "logs", "sai-find-00.jsonl")}`,
      )
      expect(result.lines).toContain("latest_sai_find_chat_count: 2")
      expect(result.lines).toContain(
        `latest_sai_bootstrap: ${join(dir, "logs", "sai-bootstrap-02.jsonl")}`,
      )
      expect(result.lines).toContain("exit_code: 0")
    })
  })

  test("suggests password and connection fixes for warnings and failures", async () => {
    await withTempConfig(async (dir) => {
      await writeProfiles(dir, {
        name: "corinne",
        apiId: 123,
        apiHash: "hash",
        sessionString: "session",
      })

      const result = await runHealthCheck({
        env: {
          TG_SAI_DIR: dir,
        },
        createClient: connectionFailureClientFactory,
      })

      expect(result.exitCode).toBe(1)
      expect(result.lines).toContain(
        "❌😭 telegram_connection: failed AUTH_KEY_UNREGISTERED",
      )
      expect(result.lines).toContain("⚠️ password_status: missing")
      expect(result.lines).toContain("suggested_fixes:")
      expect(result.lines).toContain("- telegram_connection (error):")
      expect(result.lines).toContain(
        "  Check the active profile API credentials and Telegram session string.",
      )
      expect(result.lines).toContain("  tg-sai profile show corinne")
      expect(result.lines).toContain("- password_status (warning):")
      expect(result.lines).toContain(
        "  tg-sai profile add --name corinne --password '<password>'",
      )
    })
  })
})
