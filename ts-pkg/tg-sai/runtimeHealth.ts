import { createHash } from "node:crypto"
import { gzipSync, inflateSync } from "node:zlib"
import { createTelegramCryptoProvider, verifyRuntimeAssets } from "./telegramRuntime"
import { createMtcuteClient, createMtcuteRuntimeClient } from "./mtcuteAdapter"
import { mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { configDir, strictProfileCredentials } from "./profiles"
import { accountPaths, withAccountLock } from "./provisioningRuntime"
import { withTelemetryIdentity } from "./telemetry/middleware"
import type { ClientTelemetry } from "./telemetry/middleware"

export const HEALTH_CUSTODY_USER_ID = 8274304175
export interface RuntimeHealthCheck {
  name: string
  ok: boolean
  reason?: string
}
export interface RuntimeHealthResult {
  ok: boolean
  mode: "offline" | "online"
  checks: RuntimeHealthCheck[]
  userId?: number
}
export interface RuntimeHealthOptions {
  online?: boolean
  custodyProfile?: string
  timeoutMs?: number
  /** Records the online getMe; identity is filled in for the custody user. */
  telemetry?: ClientTelemetry
}

async function bounded<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("Health operation timed out")), timeoutMs)
      }),
    ])
  } finally {
    clearTimeout(timer)
  }
}

/** Initialize the same provider passed to TelegramClient, without credentials or I/O to Telegram. */
export async function checkOfflineTelegramRuntime(): Promise<RuntimeHealthResult> {
  const checks: RuntimeHealthCheck[] = []
  try {
    await verifyRuntimeAssets()
    const crypto = createTelegramCryptoProvider()
    await crypto.initialize()
    checks.push({ name: "telegram_runtime_assets", ok: true })
    const key = new Uint8Array(32).fill(7)
    const iv = new Uint8Array(32).fill(3)
    const input = Uint8Array.from({ length: 32 }, (_, i) => i)
    const encrypted = crypto.createAesIge(key, iv).encrypt(input)
    const decrypted = crypto.createAesIge(key, iv).decrypt(encrypted)
    if (!Buffer.from(decrypted).equals(Buffer.from(input)) ||
        Buffer.from(encrypted).equals(Buffer.from(input)))
      throw new Error("Telegram crypto roundtrip failed")
    const sample = new TextEncoder().encode("tg-sai compiled runtime readiness")
    if (Buffer.from(crypto.sha256(sample)).toString("hex") !==
        createHash("sha256").update(sample).digest("hex"))
      throw new Error("Telegram hash check failed")
    checks.push({ name: "telegram_crypto", ok: true })
    const compressed = crypto.gzip(sample, 4096)
    if (!compressed || !Buffer.from(inflateSync(compressed)).equals(Buffer.from(sample)) ||
        !Buffer.from(crypto.gunzip(gzipSync(sample))).equals(Buffer.from(sample)))
      throw new Error("Telegram compression check failed")
    checks.push({ name: "telegram_compression", ok: true })
    const directory = await mkdtemp(join(tmpdir(), "tg-sai-runtime-health-"))
    const client = createMtcuteRuntimeClient({ apiId: 1, apiHash: "0".repeat(32),
      storage: join(directory, "session") })
    try {
      // prepare initializes the real client storage and crypto, without connect/getMe.
      await client.prepare()
      checks.push({ name: "telegram_client_prepare", ok: true })
    } finally {
      try { await client.destroy() } finally { await rm(directory, { recursive: true, force: true }) }
    }
    return { ok: true, mode: "offline", checks }
  } catch {
    // Dependency errors can contain paths and imported source. Keep readiness output bounded.
    checks.push({ name: "telegram_runtime", ok: false, reason: "telegram_runtime_initialization_failed" })
    return { ok: false, mode: "offline", checks }
  }
}

/** Explicit online mode checks only the pinned custody identity under the worker's account lock. */
export async function checkTelegramRuntimeHealth(
  options: RuntimeHealthOptions = {},
): Promise<RuntimeHealthResult> {
  const offline = await checkOfflineTelegramRuntime()
  if (!offline.ok || !options.online) return offline
  const result: RuntimeHealthResult = { ...offline, mode: "online" }
  if (!options.custodyProfile?.trim()) {
    result.ok = false
    result.checks.push({ name: "custody_identity", ok: false, reason: "explicit_custody_profile_required" })
    return result
  }
  const timeoutMs = options.timeoutMs ?? 20000
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > 60000)
    throw new Error("Health timeout must be 1000 through 60000ms")
  try {
    const credentials = await strictProfileCredentials(options.custodyProfile, HEALTH_CUSTODY_USER_ID)
    const paths = accountPaths(configDir(), HEALTH_CUSTODY_USER_ID)
    await withAccountLock(paths.lock, async () => {
      const client = await createMtcuteClient({ apiId: credentials.apiId,
        apiHash: credentials.apiHash, session: credentials.sessionString, storage: paths.storage },
        withTelemetryIdentity(options.telemetry, {
          accountId: HEALTH_CUSTODY_USER_ID, profile: options.custodyProfile }))
      try {
        const me = await bounded(client.getMe(), timeoutMs)
        if (me.id !== HEALTH_CUSTODY_USER_ID || me.isBot)
          throw new Error("Custody identity mismatch")
        result.userId = me.id
      } finally {
        await bounded(client.destroy(), 10000)
      }
    })
    result.checks.push({ name: "custody_identity", ok: true })
    return result
  } catch {
    result.ok = false
    result.checks.push({ name: "custody_identity", ok: false,
      reason: "custody_identity_check_failed_or_account_locked" })
    return result
  }
}
