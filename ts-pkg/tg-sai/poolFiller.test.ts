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
  poolChatOwned,
  poolDescription,
} from "./poolFiller"
import type {
  GroupCount,
  PoolFailure,
  PoolRegistration,
  PoolStatusReply,
} from "./poolFiller"
import { retirePoolFill } from "./poolCli"
import { descriptionRef, PROVISIONING_BOT_ID } from "./provisioning"
import type { ChatInfo, Member, TelegramProvisioner } from "./provisioning"
import {
  countGroupDialogs,
  poolRefChatMatches,
} from "./provisioningRuntime"
import { poolStatusFromCloud, CliProvisionCloud } from "./provisioningCloud"
import { applyMigrations } from "./telemetry/db"
import { BackgroundQueue } from "./state/background"
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
      poolCandidates: async (ref) => {
        this.calls.push("search")
        return [...this.chats]
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
  failures: PoolFailure[] = []
  statusCalls = 0
  registerFails = 0
  poolStatus = async () => {
    this.statusCalls++
    return this.status
  }
  poolRegister = async (input: PoolRegistration) => {
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
  const run = () =>
    fillPoolOnce({
      accountId: options.selfId ?? CUSTODY,
      owners: options.owners ?? [CUSTODY],
      journal,
      queue,
      holds,
      pacing: { gapMs: GAP, jitter: 0.3 },
      cap,
      cloud,
      telegram: (op) => op(telegram.adapter()),
      photo: async () => new Uint8Array([0xff, 0xd8, 1, 2]),
      groups,
      now: () => nowMs,
      random: () => 0.5,
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
    // Next fill waits one jittered gap (random 0.5 = exactly the gap).
    expect(f.journal.nextFillMs(CUSTODY)).toBe(T0 + GAP)
    // Counted when the session opened, before this chat existed.
    expect(f.groups.value).toEqual({ count: 200, atMs: T0 })
  })

  test("keeps 20-minute spacing and makes one chat per call", async () => {
    const f = setup()
    await f.run()
    f.advance(GAP - 1)
    expect(await f.run()).toMatchObject({ state: "paced", nextAtMs: T0 + GAP })
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
      nowMs: T0,
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
    // Already reported when it failed.
    expect(done).toMatchObject({ reported: false, fill: { status: "retired" } })
    expect(f.cloud.failures).toHaveLength(1)
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
