import { describe, expect, test } from "bun:test"
import { Database } from "bun:sqlite"
import {
  fillPoolOnce,
  hasPoolBotRights,
  MAX_CREATION_SEARCHES,
  newPoolRef,
  ownerAccounts,
  POOL_BOT_RIGHTS,
  POOL_CHAT_TITLE,
  POOL_RETRY_MS,
  POOL_STATUS_RECHECK_MS,
  PoolCap,
  REGISTER_ALERT_MS,
  poolChatOwned,
  poolDescription,
} from "./poolFiller"
import type {
  GroupCount,
  PoolFailure,
  PoolRegistration,
  PoolStatusReply,
} from "./poolFiller"
import { drainBackgroundOnce } from "./backgroundWorker"
import { backgroundView, poolAlerts, retirePoolFill } from "./poolCli"
import { descriptionRef, PROVISIONING_BOT_ID } from "./provisioning"
import type { ChatInfo, Member, TelegramProvisioner } from "./provisioning"
import {
  countGroupDialogs,
  poolRefChatMatches,
  titledDialogs,
} from "./provisioningRuntime"
import {
  CliProvisionCloud,
  dealDeskRequestKey,
  DealDeskRefusedError,
  POOL_REFUSAL_CODES,
  refusalCode,
  poolStatusFromCloud,
} from "./provisioningCloud"
import { applyMigrations } from "./telemetry/db"
import { BackgroundQueue, spacedAfterMs } from "./state/background"
import { openStateDb, STATE_MIGRATIONS } from "./state/db"
import { HoldStore } from "./state/holds"
import { PoolJournal } from "./state/pool"

const CUSTODY = 8274304175
const UNIQUE = 1912700300
const CHAT = -1003000000001
const T0 = Date.UTC(2026, 10, 10, 12, 0, 0)
const GAP = 20 * 60 * 1000

/** A Telegram account in memory: chats it created and their members. */
class FakeTelegram {
  calls: string[] = []
  chats = new Map<
    number,
    {
      info: ChatInfo
      members: Map<number, Member>
    }
  >()
  /** Errors thrown by the next call of a method, by name. */
  failNext = new Map<string, unknown>()
  /** Whether `add` makes the bot visible. */
  botShows = true
  /** Whether a timed-out create still made the chat. */
  createLands = true
  nextChatId = CHAT
  /** New chats hide history; `showHistory` works unless `historyStuck`. */
  hideHistory = false
  historyStuck = false

  constructor(readonly selfId = CUSTODY) {}

  private maybeFail(name: string): void {
    const error = this.failNext.get(name)
    if (error === undefined) return
    this.failNext.delete(name)
    if (
      name === "create" &&
      this.createLands &&
      (error as { message?: string }).message === "Request timed out"
    )
      this.makeChat(this.pendingTitle, this.pendingDescription)
    throw error
  }
  private pendingTitle = ""
  private pendingDescription = ""

  makeChat(title: string, description: string): number {
    const chatId = this.nextChatId--
    this.chats.set(chatId, {
      info: {
        title,
        description,
        chatType: "supergroup",
        isForum: false,
        isCreator: true,
        hasPhoto: false,
        hiddenHistory: this.hideHistory,
      },
      members: new Map([[this.selfId, { role: "creator", rights: {} }]]),
    })
    return chatId
  }

  adapter(): TelegramProvisioner {
    const chat = (chatId: number) => {
      const found = this.chats.get(chatId)
      if (!found)
        throw Object.assign(new Error("no chat"), { text: "CHANNEL_INVALID" })
      return found
    }
    return {
      self: async () => this.selfId,
      resolve: async () => {
        throw new Error("not used")
      },
      create: async (title, description) => {
        this.calls.push("create")
        this.pendingTitle = title
        this.pendingDescription = description
        this.maybeFail("create")
        return this.makeChat(title, description)
      },
      member: async (chatId, userId) => {
        this.calls.push("member")
        this.maybeFail("member")
        return chat(chatId).members.get(userId) ?? { role: "absent", rights: {} }
      },
      add: async (chatId, userId) => {
        this.calls.push("add")
        this.maybeFail("add")
        if (this.botShows)
          chat(chatId).members.set(userId, { role: "member", rights: {} })
      },
      promote: async () => {
        throw new Error("pool must use promoteOnce")
      },
      promoteOnce: async (chatId, userId, rights) => {
        this.calls.push("promote")
        this.maybeFail("promote")
        // Telegram drops topic rights on a non-forum supergroup.
        const { manageTopics: _dropped, ...kept } = rights
        chat(chatId).members.set(userId, { role: "admin", rights: kept })
      },
      setPhoto: async (chatId, jpeg) => {
        this.calls.push(`photo:${jpeg.length}`)
        this.maybeFail("photo")
        chat(chatId).info.hasPhoto = true
      },
      candidates: async () => [],
      verifyMarker: async () => false,
      chatInfo: async (chatId) => {
        this.calls.push("info")
        return { ...chat(chatId).info }
      },
      showHistory: async (chatId) => {
        this.calls.push("history")
        if (!this.historyStuck) chat(chatId).info.hiddenHistory = false
      },
      poolCandidates: async (ref) => {
        this.calls.push("search")
        return [...this.chats]
          .filter(([, c]) => c.info.title === POOL_CHAT_TITLE)
          .filter(([, c]) => descriptionRef(c.info.description) === ref)
          .map(([id]) => id)
      },
      creatorId: async () => null,
      groupCount: async () => this.chats.size + 200,
    }
  }
}

class FakeCloud {
  status: PoolStatusReply = {
    accountId: String(CUSTODY),
    allowedNow: true,
    reason: null,
    cap: 10,
  }
  registered: PoolRegistration[] = []
  /** Errors the next `available` registrations throw, in order. */
  registerErrors: unknown[] = []
  /** Errors the next `filling` registrations throw, in order. */
  fillingErrors: unknown[] = []
  /** Every registration call, including failed ones, by state. */
  registerCalls: string[] = []
  failures: PoolFailure[] = []
  statusCalls = 0
  registerFails = 0
  poolStatus = async () => {
    this.statusCalls++
    return this.status
  }
  poolRegister = async (input: PoolRegistration) => {
    this.registerCalls.push(input.state)
    if (input.state === "filling" && this.fillingErrors.length)
      throw this.fillingErrors.shift()
    if (input.state === "available" && this.registerErrors.length)
      throw this.registerErrors.shift()
    if (input.state === "available" && this.registerFails > 0) {
      this.registerFails--
      throw new Error("Deal Desk down")
    }
    this.registered.push(input)
    return {}
  }
  poolFail = async (input: PoolFailure) => {
    this.failures.push(input)
    return {}
  }
}

function setup(options: { selfId?: number; owners?: number[] } = {}) {
  const db = openStateDb(":memory:")
  const journal = new PoolJournal(db)
  const queue = new BackgroundQueue(db)
  const holds = HoldStore.fromDb(db)
  const telegram = new FakeTelegram(options.selfId ?? CUSTODY)
  const cloud = new FakeCloud()
  const cap = new PoolCap(10)
  const groups: { value: GroupCount | null } = { value: null }
  let nowMs = T0
  let refs = 0
  let sessionError: unknown = null
  const pacing = { gapMs: GAP, jitter: 0.3 }
  const run = () =>
    fillPoolOnce({
      accountId: options.selfId ?? CUSTODY,
      owners: options.owners ?? [CUSTODY],
      journal,
      queue,
      holds,
      pacing,
      cap,
      cloud,
      telegram: async (op) => {
        if (sessionError) throw sessionError
        return op(telegram.adapter())
      },
      photo: async () => new Uint8Array([0xff, 0xd8, 1, 2]),
      groups,
      now: () => nowMs,
      newRef: () => ["3fa9c2e1", "0badc0de", "facefeed"][refs++]!,
      sleep: async () => {},
    })
  return {
    db,
    journal,
    queue,
    holds,
    telegram,
    cloud,
    cap,
    groups,
    run,
    advance: (ms: number) => {
      nowMs += ms
    },
    /** Moves the clock to when account-wide spacing next allows an add. */
    toSpaced: () => {
      nowMs = Math.max(nowMs, spacedAfterMs(queue.lastAddMs(), pacing) ?? 0)
    },
    pacing,
    failSession: (error: unknown) => {
      sessionError = error
    },
    now: () => nowMs,
  }
}

function poolAdds(db: Database): { source: string; outcome: string }[] {
  return db
    .query<{ source: string; outcome: string }, []>(
      "SELECT source, outcome FROM member_add ORDER BY id",
    )
    .all()
}

describe("fillPoolOnce", () => {
  test("fills one chat with exactly one custody add and registers it", async () => {
    const f = setup()
    const result = await f.run()
    expect(result).toMatchObject({
      state: "filled",
      ref: "3fa9c2e1",
      chatId: CHAT,
      stage: "registered",
    })
    expect(f.telegram.calls.filter((c) => c === "create")).toHaveLength(1)
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(1)
    expect(f.telegram.calls).toContain("photo:4")
    const chat = f.telegram.chats.get(CHAT)!
    expect(chat.info.title).toBe(POOL_CHAT_TITLE)
    expect(chat.info.description).toBe("Chat with the Sai team. Ref: 3fa9c2e1")
    expect(chat.members.get(PROVISIONING_BOT_ID)?.role).toBe("admin")
    expect(poolAdds(f.db)).toEqual([{ source: "pool", outcome: "added" }])
    expect(f.queue.addsToday(f.now())).toBe(1)
    expect(f.cloud.registered.map((r) => r.state)).toEqual([
      "filling",
      "available",
    ])
    const available = f.cloud.registered[1]!
    expect(available).toMatchObject({
      accountId: String(CUSTODY),
      chatId: String(CHAT),
      ref: "3fa9c2e1",
      title: POOL_CHAT_TITLE,
      botUserId: String(PROVISIONING_BOT_ID),
      photoSet: true,
      verifiedAt: new Date(T0).toISOString(),
    })
    for (const key of [
      "changeInfo",
      "deleteMessages",
      "banUsers",
      "inviteUsers",
      "pinMessages",
      "addAdmins",
    ])
      expect(available.botRights?.[key]).toBe(true)
    expect(available.botRights?.anonymous).toBeUndefined()
    expect(f.journal.get("3fa9c2e1")).toMatchObject({
      status: "done",
      stage: "registered",
      botAdds: 1,
    })
    // The next fill waits one jittered gap after this add.
    expect(await f.run()).toMatchObject({
      state: "paced",
      nextAtMs: spacedAfterMs(T0, f.pacing)!,
    })
    // Counted when the session opened, before this chat existed.
    expect(f.groups.value).toEqual({ count: 200, atMs: T0 })
  })

  test("keeps 20-minute ±30% spacing and makes one chat per call", async () => {
    const f = setup()
    await f.run()
    const spaced = spacedAfterMs(T0, f.pacing)!
    expect(spaced - T0).toBeGreaterThanOrEqual(GAP * 0.7)
    expect(spaced - T0).toBeLessThanOrEqual(GAP * 1.3)
    f.advance(spaced - T0 - 1)
    expect(await f.run()).toMatchObject({ state: "paced", nextAtMs: spaced })
    expect(f.cloud.statusCalls).toBe(1)
    f.advance(1)
    expect(await f.run()).toMatchObject({ state: "filled", ref: "0badc0de" })
    expect(f.telegram.calls.filter((c) => c === "create")).toHaveLength(2)
  })

  test("asks Deal Desk again only after a recheck delay when not allowed", async () => {
    const f = setup()
    f.cloud.status = { ...f.cloud.status, allowedNow: false, reason: "target_met" }
    expect(await f.run()).toMatchObject({
      state: "not_allowed",
      reason: "target_met",
      nextAtMs: T0 + POOL_STATUS_RECHECK_MS,
    })
    expect(await f.run()).toMatchObject({ state: "paced" })
    expect(f.cloud.statusCalls).toBe(1)
    expect(f.telegram.calls).toEqual([])
  })

  test("stops at Deal Desk's cap counted from local adds", async () => {
    const f = setup()
    f.cloud.status = { ...f.cloud.status, cap: 2 }
    f.queue.recordAdds({
      source: "setup",
      outcome: "uncertain",
      chatId: 1,
      userIds: [1, 2],
      nowMs: T0 - 2 * 3_600_000,
    })
    expect(await f.run()).toMatchObject({
      state: "capped",
      nextAtMs: Date.UTC(2026, 10, 11),
    })
    expect(f.cap.current(T0)).toBe(2)
    expect(f.telegram.calls).toEqual([])
  })

  test("falls back to the fixed cap when Deal Desk sends none", () => {
    const cap = new PoolCap(10)
    expect(cap.current(T0)).toBe(10)
    cap.observe(4, T0)
    expect(cap.current(T0 + 1000)).toBe(4)
    expect(cap.source(T0 + 1000)).toBe("deal_desk")
    cap.observe("7", T0)
    expect(cap.current(T0)).toBe(4)
    expect(cap.current(T0 + 61 * 60 * 1000)).toBe(10)
  })

  test("a saved create or invite hold stops filling before any call", async () => {
    const f = setup()
    f.holds.recordHold({
      accountId: CUSTODY,
      grp: "create",
      method: "channels.createChannel",
      source: "flood_wait",
      waitS: 600,
      nowMs: T0,
    })
    const result = await f.run()
    expect(result.state).toBe("held")
    expect(result.nextAtMs).toBeGreaterThan(T0 + 600_000)
    expect(f.cloud.statusCalls).toBe(0)
    expect(f.telegram.calls).toEqual([])
  })

  test("a flood on the bot add takes the add back and resumes without a second create", async () => {
    const f = setup()
    f.telegram.failNext.set(
      "add",
      Object.assign(new Error("flood"), { text: "FLOOD_WAIT_508", code: 420 }),
    )
    const held = await f.run()
    expect(held).toMatchObject({ state: "held", stage: "created" })
    expect(f.journal.get("3fa9c2e1")?.botAdds).toBe(0)
    expect(poolAdds(f.db)).toEqual([])
    f.advance(held.nextAtMs! - f.now() - 1)
    expect((await f.run()).state).toBe("waiting")
    f.advance(1)
    expect(await f.run()).toMatchObject({ state: "filled", ref: "3fa9c2e1" })
    expect(f.telegram.calls.filter((c) => c === "create")).toHaveLength(1)
    expect(poolAdds(f.db)).toEqual([{ source: "pool", outcome: "added" }])
  })

  test("a timed-out add counts as uncertain and is never sent again", async () => {
    const f = setup()
    f.telegram.failNext.set("add", new Error("Request timed out"))
    const waiting = await f.run()
    expect(waiting).toMatchObject({ state: "waiting", stage: "created" })
    expect(poolAdds(f.db)).toEqual([{ source: "pool", outcome: "uncertain" }])
    expect(f.journal.get("3fa9c2e1")?.botAdds).toBe(1)
    // Telegram ran the add after all.
    f.telegram.chats
      .get(CHAT)!
      .members.set(PROVISIONING_BOT_ID, { role: "member", rights: {} })
    f.advance(POOL_RETRY_MS)
    expect((await f.run()).state).toBe("filled")
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(1)
  })

  test("an absent bot is read again later, never re-added, then fails", async () => {
    const f = setup()
    f.telegram.botShows = false
    expect(await f.run()).toMatchObject({
      state: "waiting",
      error: "read_bot:TG_SAI_BOT_NOT_VISIBLE",
    })
    for (let i = 0; i < 2; i++) {
      f.advance(15 * 60 * 1000)
      expect((await f.run()).state).toBe("waiting")
    }
    f.advance(15 * 60 * 1000)
    expect(await f.run()).toMatchObject({
      state: "failed",
      error: "read_bot:TG_SAI_BOT_NOT_VISIBLE",
    })
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(1)
    expect(f.cloud.failures).toEqual([
      {
        accountId: String(CUSTODY),
        chatId: String(CHAT),
        stage: "add_bot",
        error: "TG_SAI_BOT_NOT_VISIBLE",
        floodWaitS: null,
      },
    ])
  })

  test("a failed fill blocks new fills until an operator retires it", async () => {
    const f = setup()
    f.telegram.failNext.set(
      "add",
      Object.assign(new Error("blocked"), {
        text: "BOT_GROUPS_BLOCKED",
        code: 400,
      }),
    )
    expect(await f.run()).toMatchObject({
      state: "failed",
      error: "add_bot:BOT_GROUPS_BLOCKED",
    })
    expect(poolAdds(f.db)).toEqual([{ source: "pool", outcome: "refused" }])
    f.advance(24 * 60 * 60 * 1000)
    expect(await f.run()).toMatchObject({ state: "blocked", ref: "3fa9c2e1" })
    expect(f.telegram.calls.filter((c) => c === "create")).toHaveLength(1)

    const dry = await retirePoolFill({
      journal: f.journal,
      cloud: f.cloud,
      ref: "3fa9c2e1",
      run: false,
    })
    expect(dry.dryRun).toBe(true)
    const done = await retirePoolFill({
      journal: f.journal,
      cloud: f.cloud,
      ref: "3fa9c2e1",
      run: true,
    })
    // Reported again, as the contract's operator retirement.
    expect(done).toMatchObject({ reported: true, fill: { status: "retired" } })
    expect(f.cloud.failures).toHaveLength(2)
    expect(f.cloud.failures[1]).toEqual({
      accountId: String(CUSTODY),
      chatId: String(CHAT),
      stage: "other",
      error: "operator_retired",
      floodWaitS: null,
    })
    f.toSpaced()
    expect((await f.run()).state).toBe("filled")
  })

  test("retiring an active fill with a chat tells Deal Desk", async () => {
    const f = setup()
    f.telegram.failNext.set("add", new Error("Request timed out"))
    await f.run()
    const done = await retirePoolFill({
      journal: f.journal,
      cloud: f.cloud,
      ref: "3fa9c2e1",
      run: true,
    })
    expect(done.reported).toBe(true)
    expect(f.cloud.failures[0]).toMatchObject({
      chatId: String(CHAT),
      stage: "other",
      error: "operator_retired",
    })
    await expect(
      retirePoolFill({
        journal: f.journal,
        cloud: f.cloud,
        ref: "3fa9c2e1",
        run: true,
      }),
    ).rejects.toThrow("already retired")
  })

  test("a timed-out create is found by its ref, never created again", async () => {
    const f = setup()
    f.telegram.failNext.set("create", new Error("Request timed out"))
    expect(await f.run()).toMatchObject({
      state: "waiting",
      stage: "creation_unknown",
    })
    f.advance(GAP)
    expect(await f.run()).toMatchObject({ state: "filled", chatId: CHAT })
    expect(f.telegram.calls.filter((c) => c === "create")).toHaveLength(1)
    expect(f.telegram.calls).toContain("search")
  })

  test("a timed-out create that made nothing is retired after repeated searches", async () => {
    const f = setup()
    f.telegram.createLands = false
    f.telegram.failNext.set("create", new Error("Request timed out"))
    await f.run()
    for (let i = 1; i < MAX_CREATION_SEARCHES; i++) {
      f.advance(GAP)
      expect((await f.run()).state).toBe("waiting")
    }
    f.advance(GAP)
    expect(await f.run()).toMatchObject({
      state: "retired",
      error: "find_created:creation_not_found",
    })
    f.advance(GAP)
    expect(await f.run()).toMatchObject({ state: "filled", ref: "0badc0de" })
  })

  test("a create flood retires the ref and waits out the hold", async () => {
    const f = setup()
    f.telegram.failNext.set(
      "create",
      Object.assign(new Error("flood"), { text: "FLOOD_WAIT_3600", code: 420 }),
    )
    const result = await f.run()
    expect(result).toMatchObject({
      state: "held",
      error: "create:FLOOD_WAIT_3600",
    })
    expect(f.journal.get("3fa9c2e1")?.status).toBe("retired")
    expect(f.journal.nextFillMs(CUSTODY)).toBeGreaterThan(T0 + 3_600_000)
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(0)
  })

  test("a flood on promotion waits; the bot is not added again", async () => {
    const f = setup()
    f.telegram.failNext.set(
      "promote",
      Object.assign(new Error("flood"), { text: "FLOOD_WAIT_60", code: 420 }),
    )
    const held = await f.run()
    expect(held).toMatchObject({ state: "held", stage: "bot_added" })
    f.advance(held.nextAtMs! - f.now())
    expect((await f.run()).state).toBe("filled")
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(1)
  })

  test("a refused photo leaves the chat without one and still registers", async () => {
    const f = setup()
    f.telegram.failNext.set(
      "photo",
      Object.assign(new Error("bad"), { text: "PHOTO_INVALID", code: 400 }),
    )
    expect((await f.run()).state).toBe("filled")
    expect(f.cloud.registered[1]?.photoSet).toBe(false)
  })

  test("a failed registration retries later without Telegram writes", async () => {
    const f = setup()
    f.cloud.registerFails = 1
    expect(await f.run()).toMatchObject({
      state: "waiting",
      stage: "photo_set",
      error: "register:deal_desk_failed",
    })
    f.telegram.calls = []
    f.advance(POOL_RETRY_MS)
    expect((await f.run()).state).toBe("filled")
    expect(
      f.telegram.calls.filter((c) =>
        ["create", "add", "promote"].includes(c) || c.startsWith("photo"),
      ),
    ).toEqual([])
  })

  test("refuses to run as an account outside the owner list", async () => {
    const f = setup({ selfId: UNIQUE, owners: [CUSTODY] })
    await expect(f.run()).rejects.toThrow("not a configured pool owner")
  })

  test("runs as another configured owner account", async () => {
    const f = setup({ selfId: UNIQUE, owners: [CUSTODY, UNIQUE] })
    expect((await f.run()).state).toBe("filled")
  })

  test("a chat whose description lost its ref fails the fill", async () => {
    const f = setup()
    f.telegram.failNext.set("add", new Error("Request timed out"))
    await f.run()
    f.telegram.chats.get(CHAT)!.info.description = "edited"
    f.advance(POOL_RETRY_MS)
    expect(await f.run()).toMatchObject({
      state: "failed",
      error: "verify_chat:pool_chat_mismatch",
    })
  })
})

describe("pool helpers", () => {
  test("ownerAccounts defaults to custody and parses a list", () => {
    expect(ownerAccounts(CUSTODY, {})).toEqual([CUSTODY])
    expect(ownerAccounts(CUSTODY, { TG_SAI_OWNER_ACCOUNTS: " " })).toEqual([
      CUSTODY,
    ])
    expect(
      ownerAccounts(CUSTODY, {
        TG_SAI_OWNER_ACCOUNTS: `${CUSTODY}, ${UNIQUE} ${UNIQUE}`,
      }),
    ).toEqual([CUSTODY, UNIQUE])
    expect(() =>
      ownerAccounts(CUSTODY, { TG_SAI_OWNER_ACCOUNTS: "8274304175,@unique" }),
    ).toThrow("Invalid")
    expect(() =>
      ownerAccounts(CUSTODY, {
        TG_SAI_OWNER_ACCOUNTS: String(PROVISIONING_BOT_ID),
      }),
    ).toThrow("Invalid")
  })

  test("pool refs and descriptions use the setup ref matcher", () => {
    expect(newPoolRef(() => new Uint8Array([0x3f, 0xa9, 0xc2, 0xe1]))).toBe(
      "3fa9c2e1",
    )
    expect(newPoolRef()).toMatch(/^[0-9a-f]{8}$/)
    expect(poolDescription("3fa9c2e1")).toBe(
      "Chat with the Sai team. Ref: 3fa9c2e1",
    )
    expect(descriptionRef(poolDescription("3fa9c2e1"))).toBe("3fa9c2e1")
    expect(() => poolDescription("XYZ")).toThrow()
  })

  test("poolChatOwned accepts any configured owner as creator", () => {
    const chat = {
      chatType: "supergroup",
      isForum: false,
      isCreator: false,
      description: poolDescription("3fa9c2e1"),
    }
    expect(poolChatOwned(chat, "3fa9c2e1", [CUSTODY], CUSTODY, null)).toBe(
      false,
    )
    expect(poolChatOwned(chat, "3fa9c2e1", [CUSTODY], CUSTODY, UNIQUE)).toBe(
      false,
    )
    expect(
      poolChatOwned(chat, "3fa9c2e1", [CUSTODY, UNIQUE], CUSTODY, UNIQUE),
    ).toBe(true)
    const own = { ...chat, isCreator: true }
    expect(poolChatOwned(own, "3fa9c2e1", [CUSTODY], CUSTODY)).toBe(true)
    expect(poolChatOwned(own, "0badc0de", [CUSTODY], CUSTODY)).toBe(false)
    expect(
      poolChatOwned({ ...own, isForum: true }, "3fa9c2e1", [CUSTODY], CUSTODY),
    ).toBe(false)
  })

  test("full admin rights cover Deal Desk's requirement", () => {
    expect(
      hasPoolBotRights({ role: "admin", rights: { ...POOL_BOT_RIGHTS } }),
    ).toBe(true)
    expect(
      hasPoolBotRights({
        role: "admin",
        rights: { ...POOL_BOT_RIGHTS, addAdmins: false },
      }),
    ).toBe(false)
    expect(
      hasPoolBotRights({ role: "member", rights: { ...POOL_BOT_RIGHTS } }),
    ).toBe(false)
    expect(POOL_BOT_RIGHTS.anonymous).toBe(false)
  })

  test("poolRefChatMatches and countGroupDialogs read dialogs", async () => {
    const full = {
      chatType: "supergroup",
      isForum: false,
      isCreator: true,
      bio: poolDescription("3fa9c2e1"),
    }
    expect(poolRefChatMatches(full, "3fa9c2e1")).toBe(true)
    expect(poolRefChatMatches({ ...full, isCreator: false }, "3fa9c2e1")).toBe(
      false,
    )
    async function* dialogs() {
      for (const type of ["chat", "user", "chat", "chat"])
        yield { peer: { type } }
    }
    expect(await countGroupDialogs(dialogs())).toBe(3)
  })

  test("poolStatusFromCloud keeps the fields the filler reads", () => {
    expect(
      poolStatusFromCloud({
        accountId: "8274304175",
        enabled: true,
        allowedNow: false,
        reason: "daily_cap",
        cap: 10,
        remaining: 0,
        needed: 87,
      }),
    ).toEqual({
      accountId: "8274304175",
      allowedNow: false,
      reason: "daily_cap",
      cap: 10,
      remaining: 0,
      needed: 87,
    })
    expect(() => poolStatusFromCloud({ allowedNow: "yes" })).toThrow()
    expect(() => poolStatusFromCloud({ allowedNow: true, cap: -1 })).toThrow()
  })

  test("CliProvisionCloud sends pool operations as the contract names them", async () => {
    const sent: [string, Record<string, unknown>][] = []
    const cloud = new CliProvisionCloud(async (operation, input) => {
      sent.push([operation, input])
      return operation === "pool-status"
        ? { accountId: "8274304175", allowedNow: true, reason: null, cap: 9 }
        : {}
    })
    expect((await cloud.poolStatus(CUSTODY)).cap).toBe(9)
    await cloud.poolFail({
      accountId: "8274304175",
      stage: "create",
      error: "CHANNELS_TOO_MUCH",
      floodWaitS: null,
    })
    expect(sent).toEqual([
      ["pool-status", { accountId: "8274304175" }],
      [
        "pool-fail",
        {
          accountId: "8274304175",
          stage: "create",
          error: "CHANNELS_TOO_MUCH",
          floodWaitS: null,
        },
      ],
    ])
  })
})

describe("state migration 0006", () => {
  test("keeps member adds and accepts pool adds", () => {
    const db = new Database(":memory:", { strict: true })
    applyMigrations(db, STATE_MIGRATIONS.slice(0, 5))
    db.run(
      "INSERT INTO member_add (source, outcome, chat_id, user_id, at_ms) " +
        "VALUES ('setup', 'uncertain', 1, 2, ?)",
      [T0],
    )
    applyMigrations(db, STATE_MIGRATIONS)
    const queue = new BackgroundQueue(db)
    queue.recordAdds({
      source: "pool",
      outcome: "added",
      chatId: CHAT,
      userIds: [PROVISIONING_BOT_ID],
      nowMs: T0,
    })
    expect(queue.addsToday(T0)).toBe(2)
    const journal = new PoolJournal(db)
    journal.start({ ref: "3fa9c2e1", accountId: CUSTODY, nowMs: T0 })
    expect(() =>
      journal.start({ ref: "3fa9c2e1", accountId: CUSTODY, nowMs: T0 }),
    ).toThrow()
    expect(() =>
      journal.start({ ref: "short", accountId: CUSTODY, nowMs: T0 }),
    ).toThrow()
    journal.paceFill(CUSTODY, T0 + 10)
    journal.paceFill(CUSTODY, T0 + 5)
    expect(journal.nextFillMs(CUSTODY)).toBe(T0 + 10)
  })
})

describe("pool fills share the account's add budget", () => {
  test("nothing runs while a live deal member is due", async () => {
    const f = setup()
    f.queue.enqueue({
      requestId: "deal",
      chatId: -42,
      userIds: [7],
      notBeforeMs: T0,
      nowMs: T0,
    })
    expect(await f.run()).toEqual({ state: "behind_background" })
    expect(f.cloud.statusCalls).toBe(0)
    expect(f.telegram.calls).toEqual([])
  })

  test("a member paced days out never stalls the pool, and shows as stalled", async () => {
    const f = setup()
    f.queue.enqueue({
      requestId: "deal",
      chatId: -42,
      userIds: [7],
      notBeforeMs: T0 + 10 * 86_400_000,
      nowMs: T0 - 2 * 86_400_000,
    })
    expect((await f.run()).state).toBe("filled")
    expect(backgroundView(f.queue, f.now())).toEqual({
      pending: 1,
      nextAt: new Date(T0 + 10 * 86_400_000).toISOString(),
      oldestPendingAt: new Date(T0 - 2 * 86_400_000).toISOString(),
      stalled: true,
    })
    const quiet = setup()
    expect(backgroundView(quiet.queue, T0)).toEqual({
      pending: 0,
      nextAt: null,
      oldestPendingAt: null,
      stalled: false,
    })
  })

  test("a resumed fill waits for queued deal members before its add", async () => {
    const f = setup()
    f.telegram.failNext.set(
      "add",
      Object.assign(new Error("flood"), { text: "FLOOD_WAIT_60", code: 420 }),
    )
    const held = await f.run()
    f.queue.enqueue({
      requestId: "deal",
      chatId: -42,
      userIds: [7],
      notBeforeMs: T0,
      nowMs: T0,
    })
    f.advance(held.nextAtMs! - f.now())
    expect(await f.run()).toMatchObject({
      state: "waiting",
      stage: "created",
      error: "add_bot:background_pending",
    })
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(1)
    expect(f.journal.get("3fa9c2e1")?.attempts).toBe(0)
  })

  test("background add, then pool: the fill waits one gap", async () => {
    const f = setup()
    // A background add finished 5 s ago; its row is done.
    f.queue.recordAdds({
      source: "background",
      outcome: "added",
      chatId: -42,
      userIds: [7],
      nowMs: T0 - 5000,
    })
    const spaced = spacedAfterMs(T0 - 5000, f.pacing)!
    expect(await f.run()).toEqual({ state: "paced", nextAtMs: spaced })
    expect(f.cloud.statusCalls).toBe(0)
    f.toSpaced()
    expect((await f.run()).state).toBe("filled")
  })

  test("a setup add between create and the bot add delays the bot add", async () => {
    const f = setup()
    f.telegram.failNext.set("add", Object.assign(new Error("x"), {
      text: "FLOOD_WAIT_30",
      code: 420,
    }))
    const held = await f.run()
    f.advance(held.nextAtMs! - f.now())
    f.queue.recordAdds({
      source: "setup",
      outcome: "added",
      chatId: -9,
      userIds: [1],
      nowMs: f.now(),
    })
    expect(await f.run()).toMatchObject({ state: "paced", stage: "created" })
    f.toSpaced()
    expect((await f.run()).state).toBe("filled")
  })

  test("pool add, then background: the drain waits one gap", async () => {
    const f = setup()
    await f.run()
    f.queue.enqueue({
      requestId: "deal",
      chatId: -42,
      userIds: [7],
      notBeforeMs: T0,
      nowMs: T0,
    })
    const drain = (nowMs: number) =>
      drainBackgroundOnce({
        queue: f.queue,
        holds: f.holds,
        accountId: CUSTODY,
        pacing: { ...f.pacing, dailyMax: 10 },
        telegram: async () => {
          throw new Error("drain must not call Telegram while paced")
        },
        nowMs,
        random: () => 0.5,
      })
    const spaced = spacedAfterMs(T0, f.pacing)!
    expect(await drain(T0 + 5000)).toMatchObject({
      state: "paced",
      nextAtMs: spaced,
    })
    expect(f.queue.summary().nextAtMs).toBe(spaced)
  })
})

describe("pool fill recovery", () => {
  test("hidden history is made visible once and read back", async () => {
    const f = setup()
    f.telegram.hideHistory = true
    expect((await f.run()).state).toBe("filled")
    expect(f.telegram.calls.filter((c) => c === "history")).toHaveLength(1)
    expect(f.telegram.chats.get(CHAT)!.info.hiddenHistory).toBe(false)
  })

  test("visible history needs no write", async () => {
    const f = setup()
    expect((await f.run()).state).toBe("filled")
    expect(f.telegram.calls).not.toContain("history")
  })

  test("history that stays hidden fails at the history stage", async () => {
    const f = setup()
    f.telegram.hideHistory = true
    f.telegram.historyStuck = true
    expect(await f.run()).toMatchObject({
      state: "failed",
      error: "history:history_still_hidden",
    })
    expect(f.cloud.failures[0]).toMatchObject({
      stage: "history",
      error: "history_still_hidden",
    })
    expect(f.cloud.registered.map((r) => r.state)).toEqual(["filling"])
  })

  test("a second create that went missing blocks for an operator", async () => {
    const f = setup()
    f.telegram.createLands = false
    for (const ref of ["3fa9c2e1", "0badc0de"]) {
      f.telegram.failNext.set("create", new Error("Request timed out"))
      await f.run()
      for (let i = 0; i < MAX_CREATION_SEARCHES; i++) {
        f.advance(POOL_RETRY_MS)
        await f.run()
      }
      expect(f.journal.get(ref)?.stage).toBe("creation_unknown")
      f.advance(POOL_RETRY_MS)
    }
    expect(f.journal.get("3fa9c2e1")?.status).toBe("retired")
    expect(f.journal.get("0badc0de")).toMatchObject({
      status: "failed",
      lastError: "find_created:creation_not_found",
    })
    expect(await f.run()).toMatchObject({ state: "blocked", ref: "0badc0de" })
    expect(f.telegram.calls.filter((c) => c === "create")).toHaveLength(2)
  })

  test("the ref search reads only chats with the pool title", async () => {
    async function* dialogs() {
      yield { peer: { type: "chat", title: POOL_CHAT_TITLE, id: 1 } }
      yield { peer: { type: "chat", title: "Sai <> Trader", id: 2 } }
      yield { peer: { type: "user", title: POOL_CHAT_TITLE, id: 3 } }
    }
    const seen: number[] = []
    for await (const d of titledDialogs(dialogs(), POOL_CHAT_TITLE))
      seen.push(d.peer.id)
    expect(seen).toEqual([1])
  })

  test("a fresh fill whose session never opened is retired unsent", async () => {
    const f = setup()
    f.failSession(new Error("Connection closed"))
    expect(await f.run()).toMatchObject({
      state: "retired",
      error: "create_unsent:TG_SAI_TRANSIENT",
    })
    expect(f.telegram.calls).toEqual([])
  })

  test("a crashed create is searched for even when the next session fails", async () => {
    const f = setup()
    // A crash after the create was sent: the row says `creating`, the chat exists.
    f.journal.start({ ref: "3fa9c2e1", accountId: CUSTODY, nowMs: T0 })
    f.telegram.makeChat(POOL_CHAT_TITLE, poolDescription("3fa9c2e1"))
    f.advance(1000)
    f.failSession(new Error("Connection closed"))
    expect(await f.run()).toMatchObject({ state: "waiting", stage: "creating" })
    expect(f.journal.get("3fa9c2e1")?.status).toBe("active")
    f.failSession(null)
    f.advance(POOL_RETRY_MS)
    expect(await f.run()).toMatchObject({ state: "filled", chatId: CHAT })
    expect(f.telegram.calls).toContain("search")
    expect(f.telegram.calls).not.toContain("create")
  })

  test("Deal Desk refusing `available` twice, a retry apart, fails the fill", async () => {
    const f = setup()
    f.cloud.registerErrors = [
      new DealDeskRefusedError("pool-register", "POOL_STALE_VERIFIED_AT"),
      new DealDeskRefusedError("pool-register", "POOL_STALE_VERIFIED_AT"),
    ]
    expect(await f.run()).toMatchObject({
      state: "waiting",
      error: "register:DEAL_DESK_REFUSED",
      nextAtMs: T0 + POOL_RETRY_MS,
    })
    expect(f.cloud.failures).toEqual([])
    f.advance(POOL_RETRY_MS)
    expect(await f.run()).toMatchObject({
      state: "failed",
      error: "register:POOL_STALE_VERIFIED_AT",
    })
    expect(f.cloud.failures[0]).toMatchObject({
      chatId: String(CHAT),
      stage: "register",
      error: "POOL_STALE_VERIFIED_AT",
    })
  })

  test("a single refusal followed by success registers the chat", async () => {
    const f = setup()
    f.cloud.registerErrors = [new DealDeskRefusedError("pool-register", "POOL_STALE_VERIFIED_AT")]
    expect((await f.run()).state).toBe("waiting")
    f.advance(POOL_RETRY_MS)
    expect((await f.run()).state).toBe("filled")
    expect(f.cloud.failures).toEqual([])
  })

  test("Deal Desk outages retry on their own counter", async () => {
    const f = setup()
    f.cloud.registerFails = 6
    for (let i = 1; i <= 6; i++) {
      expect((await f.run()).state).toBe("waiting")
      const fill = f.journal.get("3fa9c2e1")!
      expect(fill.registerAttempts).toBe(i)
      expect(fill.attempts).toBe(0)
      f.advance(fill.nextAttemptMs - f.now())
    }
    // A transient read after six outages still only retries.
    f.telegram.failNext.set("member", new Error("Request timed out"))
    expect((await f.run()).state).toBe("waiting")
    f.advance(POOL_RETRY_MS)
    expect((await f.run()).state).toBe("filled")
  })

  test("a fresh group count at Deal Desk's stop refuses the create", async () => {
    const f = setup()
    f.cloud.status = { ...f.cloud.status, groupStopAt: 200 }
    expect(await f.run()).toMatchObject({
      state: "not_allowed",
      reason: "group_limit",
      nextAtMs: T0 + POOL_STATUS_RECHECK_MS,
    })
    expect(f.telegram.calls).not.toContain("create")
    expect(f.groups.value?.count).toBe(200)
    f.cloud.status = { ...f.cloud.status, groupStopAt: 490 }
    f.advance(POOL_STATUS_RECHECK_MS)
    expect((await f.run()).state).toBe("filled")
  })

  test("an add error we cannot place counts as uncertain", async () => {
    for (const error of [
      new Error("unexpected"),
      Object.assign(new Error("odd"), { text: "RANDOM_THING", code: 400 }),
    ]) {
      const f = setup()
      f.telegram.failNext.set("add", error)
      expect((await f.run()).state).toBe("failed")
      expect(poolAdds(f.db)).toEqual([{ source: "pool", outcome: "uncertain" }])
      expect(f.queue.addsToday(f.now())).toBe(1)
      expect(f.journal.get("3fa9c2e1")?.botAdds).toBe(1)
    }
  })


})

describe("filling registration comes before the bot add", () => {
  test("registers filling before the add, every fill", async () => {
    const f = setup()
    expect((await f.run()).state).toBe("filled")
    expect(f.cloud.registerCalls).toEqual(["filling", "available"])
  })

  test("a registration outage waits and sends no bot add", async () => {
    const f = setup()
    f.cloud.fillingErrors = [new Error("Deal Desk down")]
    expect(await f.run()).toMatchObject({
      state: "waiting",
      stage: "created",
      error: "register:deal_desk_failed",
      nextAtMs: T0 + POOL_RETRY_MS,
    })
    expect(f.telegram.calls).toContain("create")
    expect(f.telegram.calls).not.toContain("add")
    expect(poolAdds(f.db)).toEqual([])
    expect(f.journal.get("3fa9c2e1")).toMatchObject({
      botAdds: 0,
      registerAttempts: 1,
      registerRetrySinceMs: T0,
    })
    f.advance(POOL_RETRY_MS)
    expect((await f.run()).state).toBe("filled")
    expect(f.journal.get("3fa9c2e1")).toMatchObject({
      registerAttempts: 0,
      registerRetrySinceMs: null,
    })
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(1)
  })

  test("a refusal repeated a retry apart fails the fill with no bot add", async () => {
    const f = setup()
    f.cloud.fillingErrors = [
      new DealDeskRefusedError("pool-register", "POOL_REFUSED_TRANSITION"),
      new DealDeskRefusedError("pool-register", "POOL_REFUSED_TRANSITION"),
    ]
    expect(await f.run()).toMatchObject({
      state: "waiting",
      error: "register:DEAL_DESK_REFUSED",
      nextAtMs: T0 + POOL_RETRY_MS,
    })
    f.advance(POOL_RETRY_MS)
    expect(await f.run()).toMatchObject({
      state: "failed",
      error: "register:POOL_REFUSED_TRANSITION",
    })
    expect(f.cloud.failures).toEqual([
      {
        accountId: String(CUSTODY),
        chatId: String(CHAT),
        stage: "register",
        error: "POOL_REFUSED_TRANSITION",
        floodWaitS: null,
      },
    ])
    expect(f.telegram.calls).not.toContain("add")
    expect(poolAdds(f.db)).toEqual([])
  })

  test("a resumed fill confirms registration before reading its bot", async () => {
    const f = setup()
    f.telegram.failNext.set("add", new Error("Request timed out"))
    await f.run()
    expect(f.journal.get("3fa9c2e1")?.botAdds).toBe(1)
    f.cloud.fillingErrors = [new Error("Deal Desk down")]
    f.telegram.calls = []
    f.advance(POOL_RETRY_MS)
    expect(await f.run()).toMatchObject({
      state: "waiting",
      error: "register:deal_desk_failed",
    })
    expect(f.telegram.calls).not.toContain("member")
    expect(f.telegram.calls).not.toContain("add")
    // Registered again: now the bot is read, never added a second time.
    f.telegram.chats
      .get(CHAT)!
      .members.set(PROVISIONING_BOT_ID, { role: "member", rights: {} })
    f.advance(POOL_RETRY_MS)
    expect((await f.run()).state).toBe("filled")
    expect(f.cloud.registerCalls.slice(-2)).toEqual(["filling", "available"])
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(0)
  })

  test("a day of registration retries raises an alert", async () => {
    const f = setup()
    f.cloud.registerFails = 1000
    let result = await f.run()
    for (let i = 0; i < 60 && !result.alert; i++) {
      f.advance(result.nextAtMs! - f.now())
      result = await f.run()
    }
    expect(result).toMatchObject({
      state: "waiting",
      stage: "photo_set",
      alert: "register_retrying",
      alertSinceMs: T0,
    })
    expect(f.now() - T0).toBeGreaterThanOrEqual(REGISTER_ALERT_MS)
    expect(poolAlerts(f.journal.get("3fa9c2e1"), f.now())).toEqual([
      { alert: "register_retrying", since: new Date(T0).toISOString() },
    ])
    expect(poolAlerts(f.journal.get("3fa9c2e1"), T0 + 1000)).toEqual([])
    // Success clears the run and its alert.
    f.cloud.registerFails = 0
    f.advance(result.nextAtMs! - f.now())
    expect((await f.run()).state).toBe("filled")
    expect(poolAlerts(f.journal.get("3fa9c2e1"), f.now())).toEqual([])
  })

  test("refusalCode reads only Deal Desk's stable pool codes", () => {
    // Deal Desk's CLI wraps the GraphQL errors array in its JSON error.
    const cli = (status: number, body: unknown) =>
      JSON.stringify({
        error: `GraphQL request failed (HTTP ${status}): ${JSON.stringify(body)}`,
      })
    // Transient: rate limit, credential rotation, uncoded GraphQL errors,
    // and uncoded HTTP refusals.
    expect(refusalCode(cli(429, "Request limit reached"))).toBeNull()
    expect(refusalCode(cli(401, "Invalid or revoked API credential"))).toBeNull()
    expect(
      refusalCode(cli(200, [{ message: "Query capacity reached" }])),
    ).toBeNull()
    expect(refusalCode(cli(400, [{ message: "Invalid pool transition" }]))).toBeNull()
    expect(refusalCode('{"error":"error sending request"}')).toBeNull()
    // A code word in a message is not a code.
    expect(
      refusalCode(cli(200, [{ message: "POOL_MISSING_RIGHTS later" }])),
    ).toBeNull()
    // Refusals carry `extensions.code`.
    for (const code of POOL_REFUSAL_CODES)
      expect(
        refusalCode(
          cli(200, [{ message: "refused", extensions: { code } }]),
        ),
      ).toBe(code)
    expect(refusalCode(cli(200, [{ extensions: { code: "OTHER" } }]))).toBeNull()
  })

  test("refusalCode reads only extensions.code, never message text", () => {
    const wrap = (error: string) => JSON.stringify({ error })
    // A message quoting a coded error, escaped once inside the message.
    const spoof = JSON.stringify([
      { message: 'upstream said {"code":"POOL_NOT_OWNER"}' },
    ])
    expect(
      refusalCode(wrap(`GraphQL request failed (HTTP 200): ${spoof}`)),
    ).toBeNull()
    expect(refusalCode(`GraphQL request failed (HTTP 200): ${spoof}`)).toBeNull()
    // The CLI's request-key suffix after the array is ignored.
    const coded = JSON.stringify([
      { message: "refused", extensions: { code: "POOL_MISSING_RIGHTS" } },
    ])
    expect(
      refusalCode(
        wrap(
          `GraphQL request failed (HTTP 200): ${coded}. Request key: k. ` +
            "Inspect with request status, or repeat the saved request with request retry k",
        ),
      ),
    ).toBe("POOL_MISSING_RIGHTS")
    // Raw, unwrapped output parses too; a truncated array does not.
    expect(refusalCode(`GraphQL request failed (HTTP 200): ${coded}`)).toBe(
      "POOL_MISSING_RIGHTS",
    )
    expect(
      refusalCode(
        wrap(`GraphQL request failed (HTTP 200): ${coded.slice(0, -2)}`),
      ),
    ).toBeNull()
    expect(refusalCode(wrap('{"code":"POOL_NOT_OWNER"}'))).toBeNull()
  })

  test("each pool registration gets a fresh request key; failures dedupe", () => {
    const input = { accountId: "8274304175", chatId: "-1", state: "filling" }
    expect(dealDeskRequestKey("pool-register", input)).not.toBe(
      dealDeskRequestKey("pool-register", input),
    )
    expect(dealDeskRequestKey("pool-fail", input)).toBe(
      dealDeskRequestKey("pool-fail", input),
    )
  })

  test("a chat retired between registration and the add gets no bot add", async () => {
    const f = setup()
    f.telegram.failNext.set(
      "add",
      Object.assign(new Error("flood"), { text: "FLOOD_WAIT_60", code: 420 }),
    )
    const held = await f.run()
    expect(f.journal.get("3fa9c2e1")?.botAdds).toBe(0)
    // An operator retires the chat; Deal Desk re-checks every registration.
    f.cloud.fillingErrors = [
      new DealDeskRefusedError("pool-register", "POOL_REFUSED_TRANSITION"),
      new DealDeskRefusedError("pool-register", "POOL_REFUSED_TRANSITION"),
    ]
    f.advance(held.nextAtMs! - f.now())
    expect((await f.run()).state).toBe("waiting")
    f.advance(POOL_RETRY_MS)
    expect(await f.run()).toMatchObject({
      state: "failed",
      error: "register:POOL_REFUSED_TRANSITION",
    })
    expect(f.telegram.calls.filter((c) => c === "add")).toHaveLength(1)
    expect(poolAdds(f.db)).toEqual([])
  })
})
