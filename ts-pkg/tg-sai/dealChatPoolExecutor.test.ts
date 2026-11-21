import { mkdtempSync, rmSync, statSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { describe, expect, test } from "bun:test"
import {
  DealChatPoolExecutor,
  type DealChatPoolCloud,
  type DealChatPoolResult,
  type LegacyDealChatPoolScan,
} from "./dealChatPoolExecutor"
import { managedPoolHealth, poolFillMode } from "./custodyWorker"
import {
  POOL_BOT_RIGHTS,
  POOL_RETRY_MS,
  PoolCap,
  poolDescription,
} from "./poolFiller"
import type { Member, TelegramProvisioner } from "./provisioning"
import { DealDeskRefusedError, refusalCode } from "./provisioningCloud"
import { BackgroundQueue } from "./state/background"
import { DealChatPoolOutbox } from "./state/dealChatPoolOutbox"
import { openStateDb } from "./state/db"
import { HoldStore } from "./state/holds"
import { PoolJournal } from "./state/pool"

const ACCOUNT = 8274304175
const CHAT = -1003000000001
const NOW = Date.UTC(2026, 10, 21, 12)
const REF = "aabb4455"
const EMPTY_MEMBER: Member = { role: "absent", rights: {} }

function claim(
  step: string,
  reconciliation = false,
  expires = NOW + POOL_RETRY_MS,
) {
  return {
    groupStopAt: 490,
    cap: 10,
    action: {
      accountId: String(ACCOUNT),
      fillId: "7",
      actionId: "1",
      leaseToken: "opaque-token",
      leaseExpiresAt: new Date(expires).toISOString(),
      ref: REF,
      chatId: step === "create" ? null : String(CHAT),
      step,
      reconcile: reconciliation,
    },
  }
}

function fixture(outbox = DealChatPoolOutbox.open(":memory:")) {
  const db = openStateDb(":memory:")
  const journal = new PoolJournal(db)
  const queue = new BackgroundQueue(db)
  let now = NOW
  const replies: unknown[] = []
  const results: DealChatPoolResult[] = []
  const imports: LegacyDealChatPoolScan[] = []
  const claimKeys: string[] = []
  const telegramJobs: string[] = []
  let lostClaim = false
  let lostResult = false
  let staleResult = false
  const writes: string[] = []
  let roomExists = false
  let member = EMPTY_MEMBER
  let hiddenHistory = true
  let photo = false

  const adapter: TelegramProvisioner = {
    async self() {
      return ACCOUNT
    },
    async resolve() {
      throw new Error("not used")
    },
    async candidates() {
      throw new Error("not used")
    },
    async verifyMarker() {
      throw new Error("not used")
    },
    async create() {
      writes.push("create")
      roomExists = true
      return CHAT
    },
    async member() {
      return member
    },
    async add() {
      writes.push("add")
      member = { role: "member", rights: {} }
    },
    async promote() {
      throw new Error("use promoteOnce")
    },
    async promoteOnce() {
      writes.push("promote")
      member = { role: "admin", rights: { ...POOL_BOT_RIGHTS } }
    },
    async showHistory() {
      writes.push("history")
      hiddenHistory = false
    },
    async setPhoto() {
      writes.push("photo")
      photo = true
    },
    async poolCandidates() {
      return roomExists ? [CHAT] : []
    },
    async groupCount() {
      return 17
    },
    async chatInfo() {
      return {
        title: "Sai deal chat (available)",
        description: poolDescription(REF),
        isCreator: true,
        isForum: false,
        chatType: "supergroup",
        hiddenHistory,
        hasPhoto: photo,
      }
    },
  }
  const cloud: DealChatPoolCloud = {
    async claimDealChatPoolAction(_input, key) {
      claimKeys.push(key)
      if (lostClaim) {
        lostClaim = false
        throw new Error("claim acknowledgement lost")
      }
      return (
        replies.shift() ?? {
          action: null,
          cap: 10,
          groupStopAt: 490,
          reason: "target_met",
        }
      )
    },
    async importDealChatPoolFill(input) {
      imports.push(input)
      return "complete" in input ? { managed: true } : { imported: true }
    },
    async recordDealChatPoolActionResult(result) {
      results.push(result)
      if (lostResult) {
        lostResult = false
        throw new Error("result acknowledgement lost")
      }
      if (staleResult) {
        staleResult = false
        throw new DealDeskRefusedError(
          "deal-chat-pool-result",
          "DEAL_CHAT_POOL_STALE_LEASE",
        )
      }
      return { accepted: true }
    },
  }
  const run = (holds = HoldStore.fromDb(db)) =>
    new DealChatPoolExecutor({
      accountId: ACCOUNT,
      workerId: "worker",
      cloud,
      legacy: journal,
      outbox,
      queue,
      groups: { value: null },
      cap: new PoolCap(10),
      pacing: { gapMs: 60_000, jitter: 0 },
      holds,
      photo: async () => Uint8Array.of(1, 2, 3),
      telegram: ({ fillId, actionId }, run) => {
        telegramJobs.push(`${fillId}:${actionId}`)
        return run(adapter)
      },
      now: () => now,
    })
  return {
    db,
    journal,
    outbox,
    queue,
    replies,
    results,
    imports,
    claimKeys,
    telegramJobs,
    writes,
    adapter,
    run,
    advance(ms = POOL_RETRY_MS) {
      now += ms
    },
    loseClaim() {
      lostClaim = true
    },
    loseResult() {
      lostResult = true
    },
    nextResultStale() {
      staleResult = true
    },
    setMember(value: Member) {
      member = value
    },
  }
}

describe("Rust-managed deal chat pool executor", () => {
  test("imports a legacy failed fill and acknowledges the scan before work", async () => {
    const f = fixture()
    f.journal.start({ accountId: ACCOUNT, ref: REF, nowMs: NOW })
    f.journal.update(
      REF,
      {
        stage: "bot_added",
        chatId: CHAT,
        botAdds: 1,
        status: "failed",
        lastError: "promote_bot:bot_rights_missing",
      },
      NOW,
    )
    await f.run().tick()
    expect(f.imports).toEqual([
      {
        accountId: String(ACCOUNT),
        ref: REF,
        legacyStage: "bot_added",
        legacyStatus: "failed",
        chatId: String(CHAT),
        botAdds: 1,
        errorCode: "bot_rights_missing",
      },
      { accountId: String(ACCOUNT), complete: true },
    ])
    expect(f.writes).toEqual([])
    expect(f.journal.get(REF)?.status).toBe("failed")
  })

  test("lost claim acknowledgement replays the same request key and drops an expired action", async () => {
    const f = fixture()
    const executor = f.run()
    f.loseClaim()
    f.replies.push(claim("create", false, NOW + 10_000))
    expect((await executor.tick()).reason).toBe("claim_unavailable")
    f.advance()
    await executor.tick()
    expect(f.claimKeys).toHaveLength(2)
    expect(f.claimKeys[1]).toBe(f.claimKeys[0]!)
    expect(f.results[0]).toMatchObject({
      outcome: "retry",
      errorCode: "lease_unavailable",
    })
    expect(f.telegramJobs).toEqual([])
  })

  test("restart retries a durable result receipt without making a second group", async () => {
    const dir = mkdtempSync(join(tmpdir(), "deal-chat-pool-outbox-"))
    try {
      const path = join(dir, "outbox.db")
      const original = fixture(DealChatPoolOutbox.open(path))
      original.replies.push(claim("create"))
      original.loseResult()
      expect((await original.run().tick()).reason).toBe(
        "action_result_unavailable",
      )
      original.outbox.close()
      expect(statSync(path).mode & 0o777).toBe(0o600)

      const restarted = fixture(DealChatPoolOutbox.open(path))
      expect(restarted.outbox.get(ACCOUNT)?.result).toEqual(
        original.results[0]!,
      )
      await restarted.run().tick()
      expect(restarted.results).toEqual(original.results)
      expect(restarted.outbox.get(ACCOUNT)).toBeNull()
      expect(restarted.claimKeys).toHaveLength(0)
      expect(original.writes).toEqual(["create"])
      expect(original.telegramJobs).toEqual(["7:1"])
      expect(restarted.writes).toEqual([])
      restarted.outbox.close()
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  test("stale result receipt clears without resending the Telegram operation", async () => {
    const f = fixture()
    f.replies.push(claim("create"))
    f.loseResult()
    await f.run().tick()
    f.nextResultStale()
    const restarted = f.run()
    expect((await restarted.tick()).reason).toBe("DEAL_CHAT_POOL_STALE_LEASE")
    expect(f.outbox.get(ACCOUNT)).toBeNull()
    f.advance()
    f.replies.push(claim("create", true, NOW + 2 * POOL_RETRY_MS))
    await restarted.tick()
    expect(f.results.at(-1)).toMatchObject({
      outcome: "succeeded",
      chatId: String(CHAT),
    })
    expect(f.writes).toEqual(["create"])
  })

  test("checks the lease again after preflight reads, before creating a group", async () => {
    const f = fixture()
    f.replies.push(claim("create", false, NOW + 1))
    f.adapter.groupCount = async () => {
      f.advance(2)
      return 17
    }
    await f.run().tick()
    expect(f.results.at(-1)).toMatchObject({
      outcome: "retry",
      errorCode: "lease_unavailable",
    })
    expect(f.writes).toEqual([])
  })

  test("a lost create response reconciles using read-only reference lookup", async () => {
    const f = fixture()
    const original = f.adapter.create
    f.adapter.create = async (...args) => {
      await original(...args)
      throw new Error("Request timed out")
    }
    f.replies.push(
      claim("create"),
      claim("create", true, NOW + 2 * POOL_RETRY_MS),
    )
    const executor = f.run()
    await executor.tick()
    expect(f.results[0]).toMatchObject({ outcome: "uncertain" })
    f.advance()
    await executor.tick()
    expect(f.results[1]).toMatchObject({
      outcome: "succeeded",
      chatId: String(CHAT),
    })
    expect(f.writes).toEqual(["create"])
  })

  test("an unobserved bot add is read-only with the budget charged before Telegram", async () => {
    const f = fixture()
    f.adapter.add = async () => {
      expect(f.queue.addsToday(NOW)).toBe(1)
      f.writes.push("add")
      throw new Error("Connection closed")
    }
    f.replies.push(
      claim("add_bot"),
      claim("add_bot", true, NOW + 2 * POOL_RETRY_MS),
    )
    const executor = f.run()
    await executor.tick()
    expect(f.results[0]).toMatchObject({ outcome: "uncertain" })
    f.advance()
    await executor.tick()
    expect(f.results[1]).toMatchObject({ outcome: "not_found" })
    expect(f.writes).toEqual(["add"])
    expect(f.queue.addsToday(NOW)).toBe(1)
  })

  test("promotion refusal affects only the current room", async () => {
    const f = fixture()
    f.replies.push(claim("promote_bot"), claim("create"))
    f.adapter.promoteOnce = async () => {
      throw { code: 403, text: "CHAT_ADMIN_REQUIRED" }
    }
    const executor = f.run()
    expect((await executor.tick()).state).toBe("failed")
    expect(f.results[0]).toMatchObject({
      outcome: "room_failed",
      errorCode: "CHAT_ADMIN_REQUIRED",
    })
    expect((await executor.tick()).state).toBe("progress")
    expect(f.writes).toEqual(["create"])
  })

  test("a successful promotion without visible rights schedules a read first", async () => {
    const f = fixture()
    f.replies.push(claim("promote_bot"), claim("promote_bot", true))
    f.adapter.promoteOnce = async () => {
      f.writes.push("promote")
    }
    const executor = f.run()
    await executor.tick()
    await executor.tick()
    expect(f.results.map(({ outcome }) => outcome)).toEqual([
      "uncertain",
      "not_found",
    ])
    expect(f.writes).toEqual(["promote"])
  })

  test("checks admin rights and visible history before confirming availability", async () => {
    const f = fixture()
    f.setMember({ role: "admin", rights: { ...POOL_BOT_RIGHTS } })
    f.replies.push(claim("show_history"), claim("set_photo"), claim("verify"))
    const executor = f.run()
    await executor.tick()
    await executor.tick()
    const tick = await executor.tick()
    expect(tick.state).toBe("filled")
    expect(f.results.at(-1)).toMatchObject({
      outcome: "succeeded",
      verifiedAt: new Date(NOW).toISOString(),
      photoSet: true,
      botRights: { addAdmins: true, manageCall: true },
    })
    expect(f.writes).toEqual(["history", "photo"])
  })

  test("an unreadable local hold store refuses before a Telegram write", async () => {
    const f = fixture()
    f.replies.push(claim("create"))
    await f.run(HoldStore.unavailable("state missing")).tick()
    expect(f.results[0]).toMatchObject({ outcome: "retry", errorCode: "hold" })
    expect(f.writes).toEqual([])
  })

  test("preserves account-wide pacing and reports Telegram holds", async () => {
    const f = fixture()
    f.adapter.create = async () => {
      throw { code: 420, text: "FLOOD_WAIT_300" }
    }
    f.replies.push(claim("create"))
    await f.run().tick()
    expect(f.results[0]).toMatchObject({
      outcome: "retry",
      errorCode: "FLOOD_WAIT_300",
    })
    expect(Date.parse(f.results[0]!.retryAt!)).toBeGreaterThan(NOW + 300_000)
    f.queue.recordAdds({
      source: "background",
      outcome: "added",
      chatId: CHAT,
      userIds: [2],
      nowMs: NOW,
    })
    f.replies.push(claim("add_bot"))
    await f.run().tick()
    expect(f.results[1]).toMatchObject({ outcome: "retry", errorCode: "paced" })
    expect(f.writes).toEqual([])
  })

  test("managed mode is exclusive and health omits free-form diagnostics", () => {
    expect(poolFillMode({ TG_SAI_POOL_FILL: "managed" })).toBe("managed")
    expect(poolFillMode({ TG_SAI_POOL_FILL: "1" })).toBe("legacy")
    expect(poolFillMode({})).toBe("off")
    const baseTick = {
      results: [],
      dryRun: false,
      requesterWork: false,
      nextAfter: null,
    }
    expect(
      managedPoolHealth(
        {
          ...baseTick,
          pool: {
            state: "error",
            error: "raw external message",
            nextAtMs: NOW + 1000,
          },
        },
        undefined,
        new Date(NOW).toISOString(),
      ),
    ).toEqual({
      mode: "managed",
      state: "unavailable",
      nextAt: new Date(NOW + 1000).toISOString(),
    })
    expect(
      managedPoolHealth(
        { ...baseTick, requesterWork: true },
        undefined,
        new Date(NOW).toISOString(),
      ),
    ).toEqual({
      mode: "managed",
      state: "waiting",
      reason: "waiting_on_chat_setup",
    })
    expect(
      refusalCode(
        'GraphQL request failed (HTTP 200): [{"extensions":{"code":"DEAL_CHAT_POOL_STALE_LEASE"}}]',
      ),
    ).toBe("DEAL_CHAT_POOL_STALE_LEASE")
  })
})
