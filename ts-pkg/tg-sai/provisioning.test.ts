import { describe, expect, setSystemTime, test } from "bun:test"
import { mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { MtcuteError, MtTimeoutError, tl, TransportError } from "@mtcute/bun"
import { holdExitFor } from "./main"
import { addProfile, strictProfileCredentials } from "./profiles"
import {
  AWAITING_JOIN,
  BOT_ADDS_EXHAUSTED,
  BOT_BASELINE_RIGHTS,
  BOT_NOT_VISIBLE,
  cloudRetryAt,
  creationMarker,
  definiteCreateRejection,
  isPerUserAddError,
  isTransientTelegramError,
  MEMBER_NOT_VISIBLE,
  MAX_BOT_ADDS,
  MAX_TRANSIENT_RETRIES,
  MEMBER_NOT_VISIBLE_RETRY_MS,
  MEMBER_RECHECK_MS,
  MIN_CLOUD_RETRY_MS,
  NOT_VISIBLE_INVITATION_REASON,
  PRIVACY_INVITATION_REASON,
  PROVISIONING_BOT_ID,
  provisionRetryAt,
  provisionSetup,
  QUEUE_UNAVAILABLE,
  requestFingerprint,
  safeProvisionError,
  TEAM_ADMIN_RIGHTS,
  TELEGRAM_TRANSIENT,
  TRANSIENT_LIMIT,
  TRANSIENT_RETRY_MS,
  validateRequest,
  waitingOnInvitedMember,
} from "./provisioning"
import type {
  BackgroundEnqueue,
  JournalRecord,
  Member,
  Progress,
  ProvisionCloud,
  ProvisionJournal,
  ProvisionRequest,
  Setup,
  TelegramProvisioner,
} from "./provisioning"
import {
  CliProvisionCloud,
  cloudRequest,
  dealDeskRequestKey,
} from "./provisioningCloud"
import { HoldAccountUnknownError, HoldError } from "./state/guard"
import { holdUntil, HoldStoreUnavailableError } from "./state/holds"
import {
  accountPaths,
  FileProvisionJournal,
  normalizeProvisioningMember,
  provisioningRoster,
  withAccountLock,
} from "./provisioningRuntime"

function request(): ProvisionRequest {
  return {
    requestId: "test-setup-1",
    custodyProfile: "custody",
    expectedUserId: 10,
    title: "Sai test",
    participants: ["alice", 22],
    admins: [{ userId: 22, rightsProfile: "team" }],
    botRightsProfile: "baseline",
    dealIds: ["deal-a", "deal-b"],
  }
}
function fixture() {
  let setup: Setup = { ...request(), state: "prepared" }
  let local: JournalRecord | null = null
  const events: string[] = []
  const queued: { requestId: string; chatId: number; userIds: number[] }[] =
    []
  const background: BackgroundEnqueue = {
    enqueue: (input) => {
      events.push(`queue:${input.userIds.join(",")}`)
      queued.push(structuredClone(input))
    },
  }
  const members = new Map<number, Member>([
    [10, { role: "creator", rights: {} }],
  ])
  const cloud: ProvisionCloud = {
    view: async () => structuredClone(setup),
    claim: async (_id, workerId) => {
      events.push(`claim:${workerId}`)
      setup = { ...setup, claimToken: "claim" }
      return structuredClone(setup)
    },
    progress: async (_id, _token, progress) => {
      events.push(`cloud:${progress.state}`)
      setup = { ...setup, ...progress }
      return structuredClone(setup)
    },
    resolve: async (_id, chatId) => {
      setup = { ...setup, state: "configuring", chatId }
      return structuredClone(setup)
    },
  }
  const journal: ProvisionJournal = {
    read: async () => structuredClone(local),
    write: async (_id, record) => {
      events.push(`local:${record.state}:${record.chatId ?? "none"}`)
      local = structuredClone(record)
    },
  }
  const telegram: TelegramProvisioner = {
    self: async () => 10,
    resolve: async (peer) => ({
      id: peer === "alice" ? 21 : Number(peer),
      isBot: false,
    }),
    create: async () => {
      events.push("create")
      return -100123
    },
    member: async (_chat, id) =>
      members.get(id) ?? { role: "absent", rights: {} },
    add: async (_chat, id) => {
      events.push(`add:${id}`)
      if (!members.has(id)) members.set(id, { role: "member", rights: {} })
    },
    promote: async (_chat, id, rights) => {
      events.push(`promote:${id}`)
      members.set(id, { role: "admin", rights })
    },
    candidates: async () => [-100123],
    verifyMarker: async () => true,
  }
  return {
    cloud,
    journal,
    telegram,
    events,
    members,
    queued,
    background,
    get setup() {
      return setup
    },
    set setup(value: Setup) {
      setup = value
    },
    get local() {
      return local
    },
    set local(value: JournalRecord | null) {
      local = value
    },
    run: (sleep?: (ms: number) => Promise<void>) =>
      provisionSetup({
        setup,
        cloud,
        journal,
        telegram,
        workerId: "worker-one",
        background,
        ...(sleep ? { sleep } : {}),
      }),
  }
}

describe("custody provisioning", () => {
  test("receipt verification time precedes completion link reads", async () => {
    const f = fixture()
    const auditTime = new Date("2026-11-05T05:00:00.000Z")
    try {
      setSystemTime(auditTime)
      f.telegram.completionLinks = async () => {
        setSystemTime(new Date(auditTime.getTime() + 10_000))
        return {}
      }
      expect((await f.run()).state).toBe("ready")
      expect((f.setup.receipt as { verifiedAt: string }).verifiedAt).toBe(
        auditTime.toISOString(),
      )
    } finally {
      setSystemTime()
    }
  })
  test("completion links are saved after verification and ready jobs are not rewritten", async () => {
    const f = fixture()
    let reads = 0
    f.telegram.completionLinks = async (chatId) => {
      reads++
      expect(chatId).toBe(Number(f.setup.chatId))
      expect(f.members.get(22)?.role).toBe("admin")
      return {
        openGroupUrl: "https://t.me/c/123/47",
        primaryInviteLink: "https://t.me/+fixturePrimary",
      }
    }
    const first = await f.run()
    expect(f.setup.receipt).toMatchObject({
      openGroupUrl: "https://t.me/c/123/47",
      primaryInviteLink: "https://t.me/+fixturePrimary",
    })
    const receipt = structuredClone(f.setup.receipt)
    await f.run()
    expect(f.setup.receipt).toEqual(receipt)
    expect(reads).toBe(1)
    expect(f.events.filter((event) => event === "create")).toHaveLength(1)
  })
  test("link failure does not fail verified setup or trigger another group", async () => {
    const f = fixture()
    f.telegram.completionLinks = async () => {
      throw { text: "FLOOD_WAIT_500", code: 420 }
    }
    const first = await f.run()
    expect(first.state).toBe("ready")
    expect(f.setup.receipt).not.toHaveProperty("openGroupUrl")
    expect(f.setup.receipt).not.toHaveProperty("primaryInviteLink")
    expect((await f.run()).state).toBe("ready")
    expect(f.events.filter((event) => event === "create")).toHaveLength(1)
  })
  test("a creation retry after explicit flood rejection writes a new creating transition before Telegram", async () => {
    const f = fixture()
    f.setup = {
      ...f.setup,
      participants: ["alice", "22"],
      admins: [{ userId: "22", rightsProfile: "team" }],
    }
    const receipts = new Map<string, unknown>()
    const creatingKeys: string[] = []
    const wire = () => ({
      ...cloudRequest(f.setup),
      state: f.setup.state,
      chatId: f.setup.chatId ? String(f.setup.chatId) : null,
      claimToken: "current-token",
      resolvedParticipants: f.setup.binding?.participants.map(String),
      resolvedAdmins: f.setup.binding?.admins.map(String),
    })
    const cloud = new CliProvisionCloud(async (operation, input) => {
      if (operation === "view") return wire()
      const key = dealDeskRequestKey(operation, input)
      if (operation === "progress" && input.state === "creating")
        creatingKeys.push(key)
      if (receipts.has(key)) return structuredClone(receipts.get(key))
      if (operation === "progress") {
        if (
          input.state === "configuring" &&
          !f.setup.chatId &&
          f.setup.state !== "creating"
        )
          throw new Error("Configuring requires authoritative creating intent")
        f.setup = {
          ...f.setup,
          state: input.state as Setup["state"],
          ...(input.chatId ? { chatId: Number(input.chatId) } : {}),
          ...(Array.isArray(input.resolvedParticipants)
            ? {
                binding: {
                  participants: input.resolvedParticipants.map(Number),
                  admins: (input.resolvedAdmins as string[]).map(Number),
                },
              }
            : {}),
        }
      } else if (operation !== "claim") throw new Error("Unexpected operation")
      const output = wire()
      receipts.set(key, structuredClone(output))
      return output
    })
    let attempts = 0
    const create = f.telegram.create
    f.telegram.create = async (...args) => {
      // This assertion detects a replayed receipt that did not update the DB.
      expect(f.setup.state).toBe("creating")
      if (++attempts === 1) throw { text: "FLOOD_WAIT_1", code: 420 }
      return create(...args)
    }
    const run = () =>
      provisionSetup({
        setup: f.setup,
        telegram: f.telegram,
        cloud,
        journal: f.journal,
        workerId: "worker-one",
      })
    await expect(run()).rejects.toThrow("Group creation failed")
    expect(f.setup.state).toBe("incomplete")
    expect(f.setup.chatId).toBeUndefined()
    expect((await run()).state).toBe("ready")
    expect(creatingKeys).toHaveLength(2)
    expect(new Set(creatingKeys).size).toBe(2)
    expect(attempts).toBe(2)
    expect(f.events.filter((event) => event === "create")).toHaveLength(1)
    expect(f.setup.chatId).toBe(-100123)
  })
  test("recovered claim reacquires a current token for the same journal worker without creating another chat", async () => {
    const f = fixture()
    const binding = { participants: [21, 22], admins: [22] }
    f.setup = {
      ...f.setup,
      participants: ["alice", "22"],
      admins: [{ userId: "22", rightsProfile: "team" }],
      state: "configuring",
      chatId: -100123,
      binding,
    }
    f.local = {
      fingerprint: requestFingerprint(f.setup),
      state: "configuring",
      chatId: -100123,
      binding,
      workerId: "original-worker",
    }
    let token: string | undefined
    let epoch = 0
    const receipts = new Map<string, unknown>()
    const claimKeys: string[] = []
    const wire = () => ({
      ...cloudRequest(f.setup),
      state: f.setup.state,
      chatId: String(f.setup.chatId),
      claimToken: token,
      resolvedParticipants: binding.participants.map(String),
      resolvedAdmins: binding.admins.map(String),
    })
    const cloud = new CliProvisionCloud(async (operation, input) => {
      if (operation === "view") return wire()
      const key = dealDeskRequestKey(operation, input)
      if (operation === "claim") claimKeys.push(key)
      if (receipts.has(key)) return structuredClone(receipts.get(key))
      if (operation === "claim") {
        expect(input.workerId).toBe("original-worker")
        token ??= `current-token-${++epoch}`
      } else if (operation === "progress") {
        if (input.claimToken !== token) throw new Error("Claim no longer valid")
        f.setup = { ...f.setup, state: input.state as Setup["state"] }
      } else throw new Error("Unexpected operation")
      const output = wire()
      receipts.set(key, structuredClone(output))
      return output
    })
    const first = await cloud.claim(f.setup.requestId, "original-worker")
    expect(first.claimToken).toBe("current-token-1")
    // Operator recovery revokes the old claim while preserving the known chat.
    token = undefined
    const result = await provisionSetup({
      setup: f.setup,
      telegram: f.telegram,
      cloud,
      journal: f.journal,
      workerId: "new-process-worker",
    })
    expect(result.state).toBe("ready")
    expect(claimKeys).toHaveLength(2)
    expect(new Set(claimKeys).size).toBe(2)
    expect(String(token)).toBe("current-token-2")
    expect(f.events.filter((event) => event === "create")).toHaveLength(0)
    expect(f.setup.chatId).toBe(-100123)
    const progress = {
      requestId: "id",
      claimToken: "token",
      state: "configuring",
    }
    expect(dealDeskRequestKey("progress", progress)).not.toBe(
      dealDeskRequestKey("progress", progress),
    )
    expect(dealDeskRequestKey("prepare", { requestId: "id" })).toBe(
      dealDeskRequestKey("prepare", { requestId: "id" }),
    )
  })
  test("Telegram's implicit other admin right passes the final membership audit", async () => {
    const f = fixture()
    f.telegram.promote = async (_chatId, userId, rights) => {
      // Captured mtcute permissions include this true flag for every admin.
      f.members.set(
        userId,
        normalizeProvisioningMember({
          status: "admin",
          isMember: true,
          permissions: { _: "chatAdminRights", ...rights, other: true },
        }),
      )
    }
    expect((await f.run()).state).toBe("ready")
    expect(BOT_BASELINE_RIGHTS.other).toBe(true)
    expect(TEAM_ADMIN_RIGHTS.other).toBe(true)
    expect(BOT_BASELINE_RIGHTS.addAdmins).toBe(false)
    expect(BOT_BASELINE_RIGHTS.deleteMessages).toBe(false)
    expect(BOT_BASELINE_RIGHTS.banUsers).toBe(false)
  })
  test("flushes creation intent before RPC, records identity immediately, and verifies exact privileges", async () => {
    const f = fixture()
    const result = await f.run()
    expect(result.state).toBe("ready")
    expect(f.events.indexOf("local:creating:none")).toBeLessThan(
      f.events.indexOf("create"),
    )
    expect(f.events[f.events.indexOf("create") + 1]).toBe(
      "local:configuring:-100123",
    )
    expect(f.members.get(21)?.role).toBe("member")
    expect(f.members.get(22)).toEqual({
      role: "admin",
      rights: TEAM_ADMIN_RIGHTS,
    })
    expect(f.members.get(PROVISIONING_BOT_ID)).toEqual({
      role: "admin",
      rights: BOT_BASELINE_RIGHTS,
    })
    expect(BOT_BASELINE_RIGHTS.addAdmins).toBe(false)
    expect(BOT_BASELINE_RIGHTS.deleteMessages).toBe(false)
    expect(f.setup.dealIds).toEqual(["deal-a", "deal-b"])
    expect((f.setup.receipt as { chatId: string }).chatId).toBe(
      String(f.setup.chatId),
    )
  })
  test("privacy-blocked optional member remains pending with a controlled link while required admins verify", async () => {
    const f = fixture()
    f.setup = { ...f.setup, optionalParticipants: ["alice"] }
    const add = f.telegram.add
    f.telegram.add = async (chat, id) => {
      if (id === 21) throw { text: "USER_PRIVACY_RESTRICTED" }
      return add(chat, id)
    }
    let links = 0
    f.telegram.inviteLink = async () => {
      links++
      return {
        inviteLink: "https://t.me/+fixtureLink",
        expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      }
    }
    const result = await f.run()
    expect(result.state).toBe("ready")
    expect(links).toBe(1)
    const receipt = f.setup.receipt as {
      pendingInvitations: { userId: string }[]
      participants: { userId: string; role: string }[]
    }
    expect(receipt.pendingInvitations[0]?.userId).toBe("21")
    expect(
      receipt.participants.find((member) => member.userId === "21")?.role,
    ).toBe("absent")
    expect(f.members.get(22)?.role).toBe("admin")
    expect(f.events.filter((event) => event === "create")).toHaveLength(1)
  })
  test("optional invitation never masks a flood wait or failed required admin", async () => {
    for (const error of [
      { text: "FLOOD_WAIT_30", code: 420 },
      { text: "USER_PRIVACY_RESTRICTED" },
    ]) {
      const f = fixture()
      f.setup = { ...f.setup, optionalParticipants: ["alice"] }
      f.telegram.add = async (_chat, id) => {
        if (id === 22) throw error
        f.members.set(id, { role: "member", rights: {} })
      }
      f.telegram.inviteLink = async () => {
        throw new Error("must not create a link for required admin")
      }
      await expect(f.run()).rejects.toThrow("Setup incomplete")
      expect(f.setup.state).toBe("incomplete")
      expect(f.setup.chatId).toBe(-100123)
      if (error.text === "FLOOD_WAIT_30")
        expect(Date.parse(f.setup.retryAt!)).toBeGreaterThan(Date.now())
    }
  })
  test("ready setup rechecks live custody owner and bot/admin rights", async () => {
    const f = fixture()
    await f.run()
    f.members.set(PROVISIONING_BOT_ID, { role: "member", rights: {} })
    await expect(f.run()).rejects.toThrow("rights changed")
    expect(f.events.filter((event) => event === "create")).toHaveLength(1)
  })
  test("optional participants cannot include admins or arbitrary outsiders", () => {
    expect(() =>
      validateRequest({ ...request(), optionalParticipants: [22] }),
    ).toThrow("Optional participants")
    expect(() =>
      validateRequest({ ...request(), optionalParticipants: [99] }),
    ).toThrow("Optional participants")
  })
  test("stale prepared claim replay cannot recreate a group after authoritative creation", async () => {
    const f = fixture()
    const original = { ...f.setup, claimToken: "claim" }
    f.cloud.claim = async () => {
      f.setup = {
        ...f.setup,
        state: "configuring",
        chatId: -100123,
        binding: { participants: [21, 22], admins: [22] },
      }
      return original
    }
    expect((await f.run()).state).toBe("ready")
    expect(f.events).not.toContain("create")
  })
  test("stale claim replay with creating cloud state becomes unknown without creating", async () => {
    const f = fixture()
    const original = { ...f.setup, claimToken: "claim" }
    f.cloud.claim = async () => {
      f.setup = { ...f.setup, state: "creating" }
      return original
    }
    expect(await f.run()).toEqual({
      state: "creation_unknown",
      candidates: [-100123],
    })
    expect(f.setup.state).toBe("creation_unknown")
    expect(f.events).not.toContain("create")
  })
  test("wrong account aborts before claims or Telegram writes", async () => {
    const f = fixture()
    f.telegram.self = async () => 99
    await expect(f.run()).rejects.toThrow("identity mismatch")
    expect(f.events).toEqual([])
  })
  test("transport timeout leaves unknown creation and never creates on resume", async () => {
    const f = fixture()
    f.telegram.create = async () => {
      f.events.push("create")
      throw new Error("network timeout")
    }
    await expect(f.run()).rejects.toThrow("Group creation failed")
    expect(f.setup.state).toBe("creation_unknown")
    expect(f.setup).not.toHaveProperty("creationRejected")
    expect(await f.run()).toEqual({
      state: "creation_unknown",
      candidates: [-100123],
    })
    expect(f.events.filter((e) => e === "create")).toHaveLength(1)
  })
  test("a process crash during creating becomes unknown, with known private result as candidate", async () => {
    const f = fixture()
    f.setup = {
      ...f.setup,
      state: "creating",
      binding: { participants: [21, 22], admins: [22] },
    }
    f.local = {
      fingerprint: requestFingerprint(f.setup),
      state: "configuring",
      chatId: -100123,
      workerId: "original",
    }
    expect(await f.run()).toEqual({
      state: "creation_unknown",
      candidates: [-100123],
    })
    expect(f.setup.state).toBe("creation_unknown")
    expect(f.events).not.toContain("create")
  })
  test("privacy failures retain group and frozen IDs; resume does not re-resolve handles", async () => {
    const f = fixture()
    const add = f.telegram.add
    f.telegram.add = async (chat, id) => {
      // A known refusal: the add did not happen, so a resume may add again.
      if (id === 22)
        throw Object.assign(new Error("privacy"), {
          code: 403,
          text: "USER_PRIVACY_RESTRICTED",
        })
      await add(chat, id)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.setup.state).toBe("incomplete")
    expect(f.setup.chatId).toBe(-100123)
    f.telegram.add = add
    f.telegram.resolve = async () => {
      throw new Error("must use bound identity")
    }
    expect((await f.run()).state).toBe("ready")
    expect(f.events.filter((e) => e === "create")).toHaveLength(1)
    expect(f.events.filter((e) => e === "add:21")).toHaveLength(1)
    expect(f.events.filter((e) => e.startsWith("claim:"))).toEqual([
      "claim:worker-one",
      "claim:worker-one",
    ])
  })
  test("flood waits preserve incomplete state and sanitized retry duration", async () => {
    const f = fixture()
    f.telegram.add = async () => {
      throw Object.assign(new Error("unsafe context"), {
        text: "FLOOD_WAIT_%d",
        seconds: 40,
        code: 420,
      })
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.setup.state).toBe("incomplete")
    expect((f.setup as Setup & { error: string }).error).toBe(
      "add_member:FLOOD_WAIT_40",
    )
    expect(safeProvisionError(new Error("session string"))).toBe(
      "provisioning_step_failed",
    )
  })
  test("failed setup records the exact operation without copying private error bodies", async () => {
    // A fresh chat reads no member before the add.
    const cases = [
      ["verify_marker", "verifyMarker"],
      ["verify_added_member", "member"],
      ["add_member", "add"],
      ["promote_admin", "promote"],
    ] as const
    for (const [step, method] of cases) {
      const f = fixture()
      f.telegram[method] = async () => {
        throw new Error("private-session-and-link")
      }
      await expect(f.run()).rejects.toThrow("incomplete")
      expect((f.setup as Setup & { error: string }).error).toBe(
        `${step}:provisioning_step_failed`,
      )
      expect(f.setup.chatId).toBe(-100123)
      expect(f.events.filter((event) => event === "create")).toHaveLength(1)
    }
    expect(safeProvisionError({ text: "SECRET_SESSION_VALUE" })).toBe(
      "provisioning_step_failed",
    )
    expect(safeProvisionError({ text: "USER_NOT_PARTICIPANT" })).toBe(
      "USER_NOT_PARTICIPANT",
    )
  })
  test("missing privileges prevent readiness", async () => {
    const f = fixture()
    f.telegram.promote = async () => {}
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.setup.state).toBe("incomplete")
  })
  test("unexpected admin on an ordinary participant prevents readiness", async () => {
    const f = fixture()
    f.members.set(21, { role: "admin", rights: TEAM_ADMIN_RIGHTS })
    await expect(f.run()).rejects.toThrow("incomplete")
  })
  test("ambiguous ready response reads authoritative committed result without regression", async () => {
    const f = fixture()
    const progress = f.cloud.progress
    f.cloud.progress = async (id, token, value) => {
      const result = await progress(id, token, value)
      if (value.state === "ready") throw new Error("lost response")
      return result
    }
    expect((await f.run()).state).toBe("ready")
    expect(f.events).not.toContain("cloud:incomplete")
  })
  test("binding preserves cloud counts and requires listed admins to be participants", async () => {
    const f = fixture()
    f.setup = { ...f.setup, participants: ["alice"] }
    await expect(f.run()).rejects.toThrow("Invalid account binding")
    expect(f.events).not.toContain("create")
  })
  test("binding rejects two handles for the same user and custody-account participants", async () => {
    for (const participants of [
      ["alice", 21],
      [10, 22],
    ]) {
      const f = fixture()
      f.setup = { ...f.setup, participants, admins: [] }
      await expect(f.run()).rejects.toThrow("Invalid account binding")
      expect(f.events).not.toContain("create")
    }
  })
  test("an explicit flood rejection permits creation retry without duplicating a successful group", async () => {
    const f = fixture()
    const create = f.telegram.create
    f.telegram.create = async () => {
      throw Object.assign(new Error("rate limit"), {
        code: 420,
        text: "FLOOD_WAIT_%d",
        seconds: 1,
      })
    }
    await expect(f.run()).rejects.toThrow("Group creation failed")
    expect(f.setup.state).toBe("incomplete")
    expect(
      (f.setup as Setup & { creationRejected?: boolean }).creationRejected,
    ).toBe(true)
    f.telegram.create = create
    expect((await f.run()).state).toBe("ready")
    expect(f.events.filter((e) => e === "create")).toHaveLength(1)
  })
  test("invalid bot participant fails before creation", async () => {
    const f = fixture()
    f.telegram.resolve = async () => ({ id: PROVISIONING_BOT_ID, isBot: true })
    await expect(f.run()).rejects.toThrow("user accounts")
    expect(f.events).not.toContain("create")
  })
})

describe("account and cloud boundaries", () => {
  test("strict profile ignores complete environment fallback and rejects unknown IDs", async () => {
    const root = await mkdtemp(join(tmpdir(), "tg-provision-profile-"))
    const env = {
      TG_SAI_DIR: root,
      TELEGRAM_API_ID: "999",
      TELEGRAM_API_HASH: "environment-hash",
      TELEGRAM_SESSION_STRING: "environment-session",
    }
    try {
      await addProfile(
        {
          name: "custody",
          apiId: 123,
          apiHash: "profile-hash",
          sessionString: "profile-session",
          password: "",
          handle: "custody",
          userId: 10,
          displayName: "Custody",
        },
        env,
      )
      expect(await strictProfileCredentials("custody", 10, env)).toEqual({
        apiId: 123,
        apiHash: "profile-hash",
        sessionString: "profile-session",
      })
      await expect(
        strictProfileCredentials("missing", 10, env),
      ).rejects.toThrow("not configured")
      await expect(
        strictProfileCredentials("custody", 99, env),
      ).rejects.toThrow("mismatch")
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
  test("account paths isolate sessions and lock excludes concurrent workers", async () => {
    const root = await mkdtemp(join(tmpdir(), "tg-provision-lock-"))
    try {
      const one = accountPaths(root, 10),
        two = accountPaths(root, 20)
      expect(one.storage).not.toBe(two.storage)
      await withAccountLock(one.lock, async () => {
        await expect(withAccountLock(one.lock, async () => {})).rejects.toThrow(
          "locked",
        )
        await withAccountLock(two.lock, async () => {})
      })
      await withAccountLock(one.lock, async () => {})
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
  test("kernel custody lock is released after an executor process is killed", async () => {
    const root = await mkdtemp(join(tmpdir(), "tg-provision-crash-"))
    const lock = accountPaths(root, 10).lock
    const script = `import {withAccountLock} from ${JSON.stringify(join(import.meta.dir, "provisioningRuntime.ts"))}; await withAccountLock(${JSON.stringify(lock)}, async()=>{console.log("held"); await new Promise(()=>{})})`
    const child = Bun.spawn([process.execPath, "-e", script], {
      stdout: "pipe",
      stderr: "pipe",
    })
    try {
      const reader = child.stdout.getReader()
      const first = await reader.read()
      reader.releaseLock()
      expect(new TextDecoder().decode(first.value).trim()).toBe("held")
      await expect(withAccountLock(lock, async () => {})).rejects.toThrow(
        "locked",
      )
      child.kill("SIGKILL")
      await child.exited
      // The guard subprocess observes stdin EOF after the parent dies.
      let acquired = false
      for (let i = 0; i < 30 && !acquired; i++) {
        try {
          await withAccountLock(lock, async () => {
            acquired = true
          })
        } catch {
          await new Promise((resolve) => setTimeout(resolve, 10))
        }
      }
      expect(acquired).toBe(true)
    } finally {
      if (child.exitCode === null) {
        child.kill("SIGKILL")
        await child.exited
      }
      await rm(root, { recursive: true, force: true })
    }
  })
  test("journal safely replaces private recovery receipts", async () => {
    const root = await mkdtemp(join(tmpdir(), "tg-provision-journal-"))
    try {
      const journal = new FileProvisionJournal(root)
      expect(await journal.read("new")).toBeNull()
      await journal.write("new", { fingerprint: "fp", state: "creating" })
      await journal.write("new", {
        fingerprint: "fp",
        state: "configuring",
        chatId: -100123,
      })
      expect(await journal.read("new")).toEqual({
        fingerprint: "fp",
        state: "configuring",
        chatId: -100123,
      })
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
  test("cloud adapter binds numeric strings and emits exact typed contract", async () => {
    const calls: { operation: string; input: Record<string, unknown> }[] = []
    const requestValue = request()
    const cloud = new CliProvisionCloud(async (operation, input) => {
      calls.push({ operation, input })
      return {
        ...cloudRequest(requestValue),
        state: operation === "progress" ? input.state : "prepared",
        chatId: null,
        resolvedParticipants: ["21", "22"],
        resolvedAdmins: ["22"],
        claimToken: "token",
      }
    })
    const result = await cloud.prepare(requestValue)
    expect(calls[0]?.input.expectedUserId).toBe("10")
    expect(
      cloudRequest({
        ...requestValue,
        state: "prepared",
        claimToken: "secret",
      } as ProvisionRequest),
    ).not.toHaveProperty("claimToken")
    expect(calls[0]?.input.admins).toEqual([
      { userId: "22", rightsProfile: "team" },
    ])
    expect(result.binding).toEqual({ participants: [21, 22], admins: [22] })
    await cloud.progress(requestValue.requestId, "token", {
      state: "incomplete",
      binding: result.binding,
      creationRejected: true,
      error: "FLOOD_WAIT_40",
    })
    expect(calls[1]?.input.resolvedParticipants).toEqual(["21", "22"])
    expect(calls[1]?.input).not.toHaveProperty("binding")
    expect(calls[1]?.input.creationRejected).toBe(true)
  })
  test("request validator rejects privilege profiles and duplicate deals", () => {
    expect(() =>
      validateRequest({
        ...request(),
        botRightsProfile: "broad",
      } as unknown as ProvisionRequest),
    ).toThrow("botRightsProfile")
    expect(() =>
      validateRequest({ ...request(), dealIds: ["same", "same"] }),
    ).toThrow("duplicate")
    expect(() =>
      validateRequest({
        ...request(),
        admins: [{ userId: 22, rightsProfile: "owner" }],
      } as unknown as ProvisionRequest),
    ).toThrow("admins")
    expect(creationMarker(request())).not.toContain("deal-a")
    expect(() =>
      validateRequest({ ...request(), requestId: "not.allowed" }),
    ).toThrow("requestId")
    expect(() =>
      validateRequest({ ...request(), participants: ["four"] }),
    ).toThrow("participants")
    expect(() =>
      validateRequest({ ...request(), title: "x".repeat(129) }),
    ).toThrow("title")
  })
})

describe("flood and hold retry times", () => {
  const now = Date.parse("2026-11-06T00:00:00Z")
  const after = (ms: number) => new Date(now + ms).toISOString()
  test("Telegram floods and local holds map to retryAt", () => {
    const hold = new HoldError(now + 90_500, "create", now)
    expect(hold.text).toBe("TG_SAI_HOLD_91")
    const cases: [unknown, string | null][] = [
      // Telegram waits get the local hold's margin: max(10 s, 1 %).
      [{ text: "FLOOD_WAIT_30", code: 420 }, after(40_000)],
      [{ text: "FLOOD_WAIT_%d", seconds: 40, code: 420 }, after(50_000)],
      [{ text: "FLOOD_WAIT_3600", code: 420 }, after(3_636_000)],
      // 12 s + 10 s margin is under the 30 s floor.
      [{ text: "FLOOD_PREMIUM_WAIT_12", code: 420 }, after(30_000)],
      [{ text: "PEER_FLOOD", code: 400 }, after(24 * 3_600_000 * 1.01)],
      // The hold's exact end wins over its rounded seconds.
      [hold, after(90_500)],
      [{ text: "TG_SAI_HOLD_45", code: 420 }, after(45_000)],
      // A hold about to end still gets the 30 s floor.
      [new HoldError(now + 2_000, "create", now), after(30_000)],
      [{ text: "TG_SAI_HOLD_1", code: 420 }, after(30_000)],
      [new HoldStoreUnavailableError("locked"), after(5 * 60_000)],
      [{ text: "USER_PRIVACY_RESTRICTED", code: 403 }, null],
      [{ text: "FLOOD_WAIT_0", code: 420 }, null],
      [new Error("FLOOD_WAIT_30"), null],
    ]
    for (const [error, expected] of cases)
      expect(provisionRetryAt(error, now)).toBe(expected)
  })
  test("cloudRetryAt never precedes the local hold or now + 30 s", () => {
    expect(cloudRetryAt(now + 90_000, now)).toBe(after(90_000))
    expect(cloudRetryAt(now + 29_999, now)).toBe(after(30_000))
    expect(cloudRetryAt(now - 5_000, now)).toBe(after(30_000))
    expect(cloudRetryAt(now + 400.2, now)).toBe(after(30_000))
    expect(cloudRetryAt(now + 2 * 31_536_000_000, now)).toBeNull()
    expect(cloudRetryAt(Number.NaN, now)).toBeNull()
    // A saved Telegram wait: the cloud time is the local hold's end or later.
    for (const waitS of [1, 30, 400, 15_382, 86_400]) {
      const retryAt = Date.parse(
        provisionRetryAt({ text: `FLOOD_WAIT_${waitS}`, code: 420 }, now)!,
      )
      expect(retryAt).toBeGreaterThanOrEqual(holdUntil(now, waitS))
      expect(retryAt).toBeGreaterThanOrEqual(now + MIN_CLOUD_RETRY_MS)
    }
  })
  test("error codes stay code-only and holds count as definite rejections", () => {
    const hold = new HoldError(now + 90_000, "create", now)
    const unavailable = new HoldStoreUnavailableError("/private/path/state.db")
    expect(safeProvisionError(hold)).toBe("TG_SAI_HOLD_90")
    expect(safeProvisionError(unavailable)).toBe(
      "TG_SAI_HOLD_STATE_UNAVAILABLE",
    )
    expect(safeProvisionError({ text: "PEER_FLOOD" })).toBe("PEER_FLOOD")
    expect(safeProvisionError({ text: "FLOOD_PREMIUM_WAIT_7" })).toBe(
      "FLOOD_PREMIUM_WAIT_7",
    )
    expect(safeProvisionError({ text: "TG_SAI_HOLD_x" })).toBe(
      "provisioning_step_failed",
    )
    for (const error of [
      hold,
      unavailable,
      { text: "PEER_FLOOD", code: 400 },
      { text: "FLOOD_PREMIUM_WAIT_7", code: 420 },
    ])
      expect(definiteCreateRejection(error)).toBe(true)
    // A hold text without the guard's code is not proof of a refusal.
    expect(definiteCreateRejection({ text: "TG_SAI_HOLD_9", code: 500 })).toBe(
      false,
    )
    expect(
      definiteCreateRejection({
        text: "TG_SAI_HOLD_STATE_UNAVAILABLE",
        code: 420,
      }),
    ).toBe(false)
  })
  test("a held creation is rejected with the hold's end as retryAt", async () => {
    const f = fixture()
    const untilMs = Date.now() + 120_000
    f.telegram.create = async () => {
      throw new HoldError(untilMs, "create")
    }
    const error = await f.run().catch((e: unknown) => e)
    expect(error).toBeInstanceOf(Error)
    expect((error as Error).message).toContain("Group creation failed")
    // The cause survives the rethrow, so `provision run` exits 75.
    expect((error as Error).cause).toBeInstanceOf(HoldError)
    expect(holdExitFor(error)?.exitCode).toBe(75)
    expect(f.setup.state).toBe("incomplete")
    expect(f.setup.retryAt).toBe(new Date(untilMs).toISOString())
    expect((f.setup as Setup & { error: string }).error).toMatch(
      /^TG_SAI_HOLD_\d+$/,
    )
    expect(
      (f.setup as Setup & { creationRejected?: boolean }).creationRejected,
    ).toBe(true)
  })
})

describe("batched member adds", () => {
  function batched(f: ReturnType<typeof fixture>) {
    const batches: number[][] = []
    f.setup = { ...f.setup, optionalParticipants: ["alice"] }
    f.telegram.inviteLink = async (_chat, userId) => {
      f.events.push(`link:${userId}`)
      return {
        inviteLink: "https://t.me/+fixtureLink",
        expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      }
    }
    return batches
  }
  test("one add call covers required users and the bot; optional users are queued with a link", async () => {
    const f = fixture()
    const batches = batched(f)
    f.telegram.addMany = async (_chat, ids) => {
      batches.push(ids)
      for (const id of ids) f.members.set(id, { role: "member", rights: {} })
      return []
    }
    f.telegram.add = async () => {
      throw new Error("must batch")
    }
    expect((await f.run()).state).toBe("ready")
    expect(batches).toEqual([[22, PROVISIONING_BOT_ID]])
    expect(f.events.filter((e) => e.startsWith("link:"))).toEqual(["link:21"])
    const receipt = f.setup.receipt as {
      pendingInvitations: Record<string, string>[]
      participants: { userId: string; role: string }[]
    }
    expect(receipt.pendingInvitations).toEqual([
      {
        userId: "21",
        inviteLink: "https://t.me/+fixtureLink",
        expiresAt: expect.any(String),
        reason: "background_queue",
      },
    ])
    expect(receipt.participants.map((p) => [p.userId, p.role])).toEqual([
      ["21", "absent"],
      ["22", "admin"],
    ])
  })
  test("a privacy error on the batch falls back to per-user adds", async () => {
    const f = fixture()
    const batches = batched(f)
    f.telegram.addMany = async (_chat, ids) => {
      batches.push(ids)
      throw { text: "USER_NOT_MUTUAL_CONTACT", code: 400 }
    }
    const add = f.telegram.add
    f.telegram.add = async (chat, id) => {
      if (id === 21) throw { text: "USER_PRIVACY_RESTRICTED", code: 403 }
      return add(chat, id)
    }
    expect((await f.run()).state).toBe("ready")
    expect(batches).toHaveLength(1)
    expect(f.events.filter((e) => /^(add|link):/.test(e))).toEqual([
      `add:22`,
      `add:${PROVISIONING_BOT_ID}`,
      "link:21",
    ])
  })
  test("a per-user 400 on the batch falls back to per-user adds", async () => {
    const f = fixture()
    const batches = batched(f)
    f.telegram.addMany = async (_chat, ids) => {
      batches.push(ids)
      throw { text: "USER_CHANNELS_TOO_MUCH", code: 400 }
    }
    const add = f.telegram.add
    f.telegram.add = async (chat, id) => {
      if (id === 22) throw { text: "USER_CHANNELS_TOO_MUCH", code: 400 }
      return add(chat, id)
    }
    const error = await f.run().catch((e: unknown) => e)
    expect((error as Error).message).toContain("Setup incomplete")
    expect(batches).toHaveLength(1)
    // The other users are still added; the refused one is not retried.
    expect(f.events.filter((e) => /^(add|link):/.test(e))).toEqual([
      `add:${PROVISIONING_BOT_ID}`,
    ])
    expect(f.queued).toEqual([])
    expect((f.setup as Setup & { error: string }).error).toBe(
      "add_member:USER_CHANNELS_TOO_MUCH",
    )
    expect(f.setup.retryAt).toBeNull()
  })
  test("per-user add errors: USER_*, CHAT_*, BOT_GROUPS_BLOCKED, privacy", () => {
    for (const text of [
      "USER_CHANNELS_TOO_MUCH",
      "USER_BANNED_IN_CHANNEL",
      "USER_KICKED",
      "CHAT_MEMBER_ADD_FAILED",
      "BOT_GROUPS_BLOCKED",
    ])
      expect(isPerUserAddError({ text, code: 400 })).toBe(true)
    expect(isPerUserAddError({ text: "USER_PRIVACY_RESTRICTED", code: 403 }))
      .toBe(true)
    for (const error of [
      { text: "FLOOD_WAIT_30", code: 420 },
      { text: "PEER_FLOOD", code: 400 },
      { text: "USER_CHANNELS_TOO_MUCH", code: 403 },
      { text: "CHANNELS_TOO_MUCH", code: 400 },
      new HoldError(Date.now() + 5_000, "invite"),
      new HoldStoreUnavailableError("locked"),
      new Error("USER_KICKED"),
    ])
      expect(isPerUserAddError(error)).toBe(false)
  })
  test("a required user's privacy refusal still adds the others, then waits on a link", async () => {
    const f = fixture()
    batched(f)
    f.telegram.addMany = async (_chat, ids) => {
      for (const id of ids)
        if (id !== 22) f.members.set(id, { role: "member", rights: {} })
      return [22]
    }
    await expect(f.run()).rejects.toThrow("Setup incomplete")
    expect(f.members.get(21)).toBeUndefined()
    expect(f.members.get(PROVISIONING_BOT_ID)?.role).toBe("member")
    expect(f.queued).toEqual([])
    expect((f.setup as Setup & { error: string }).error).toBe(
      "add_member:TG_SAI_AWAITING_JOIN",
    )
    expect(f.setup.retryAt).toBeTruthy()
    expect(
      f.local?.pendingInvitations?.map((i) => [i.userId, i.reason]),
    ).toEqual([["22", PRIVACY_INVITATION_REASON]])
  })
  test("without invitation links a required user's privacy refusal fails", async () => {
    const f = fixture()
    f.telegram.add = async (_chat, id) => {
      if (id === 22) throw { text: "USER_PRIVACY_RESTRICTED", code: 403 }
      f.members.set(id, { role: "member", rights: {} })
    }
    await expect(f.run()).rejects.toThrow("Setup incomplete")
    expect((f.setup as Setup & { error: string }).error).toBe(
      "add_member:USER_PRIVACY_RESTRICTED",
    )
    expect(f.setup.retryAt).toBeNull()
  })
  test("a flood or hold on the batch propagates its retry time without per-user adds", async () => {
    for (const error of [
      { text: "FLOOD_WAIT_%d", seconds: 50, code: 420 },
      new HoldError(Date.now() + 50_000, "invite"),
    ]) {
      const f = fixture()
      batched(f)
      f.telegram.addMany = async () => {
        throw error
      }
      f.telegram.add = async () => {
        throw new Error("must not retry per user")
      }
      await expect(f.run()).rejects.toThrow("Setup incomplete")
      expect(f.setup.state).toBe("incomplete")
      expect(Date.parse(f.setup.retryAt!)).toBeGreaterThan(Date.now() + 40_000)
      expect((f.setup as Setup & { error: string }).error).toMatch(
        /^add_member:(?:FLOOD_WAIT_50|TG_SAI_HOLD_\d+)$/,
      )
      expect(f.events.filter((e) => e.startsWith("link:"))).toEqual([])
    }
  })
  test("members already present are not added again", async () => {
    const f = fixture()
    const batches = batched(f)
    f.members.set(21, { role: "member", rights: {} })
    f.telegram.addMany = async (_chat, ids) => {
      batches.push(ids)
      for (const id of ids) f.members.set(id, { role: "member", rights: {} })
      return []
    }
    expect((await f.run()).state).toBe("ready")
    expect(batches).toEqual([[22, PROVISIONING_BOT_ID]])
    expect(f.setup.receipt).not.toHaveProperty("pendingInvitations")
  })
})

describe("requester-first setup", () => {
  function withOptional(f: ReturnType<typeof fixture>) {
    f.setup = {
      ...f.setup,
      participants: ["alice", 22, 33],
      optionalParticipants: ["alice", 33],
    }
    let links = 0
    f.telegram.inviteLink = async (_chat, userId) => {
      links++
      f.events.push(`link:${userId}`)
      return {
        inviteLink: "https://t.me/+sharedLink",
        expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      }
    }
    f.telegram.members = async () => {
      f.events.push("list")
      return new Map(f.members)
    }
    const member = f.telegram.member
    f.telegram.member = async (chat, id) => {
      f.events.push(`read:${id}`)
      return member(chat, id)
    }
    return { links: () => links }
  }
  test("adds only required users and the bot, then reports ready with queued members", async () => {
    const f = fixture()
    const counts = withOptional(f)
    expect((await f.run()).state).toBe("ready")
    const writes = f.events.filter((e) =>
      /^(add|promote|queue|link|list|cloud:ready)/.test(e),
    )
    expect(writes).toEqual([
      "add:22",
      `add:${PROVISIONING_BOT_ID}`,
      "promote:22",
      `promote:${PROVISIONING_BOT_ID}`,
      "list",
      "queue:21,33",
      "link:21",
      "cloud:ready",
    ])
    expect(counts.links()).toBe(1)
    expect(f.queued).toEqual([
      { requestId: "test-setup-1", chatId: -100123, userIds: [21, 33] },
    ])
    const receipt = f.setup.receipt as {
      pendingInvitations: { userId: string; inviteLink: string; reason: string }[]
      backgroundMembers: { userId: string; state: string }[]
    }
    expect(receipt.backgroundMembers).toEqual([
      { userId: "21", state: "pending" },
      { userId: "33", state: "pending" },
    ])
    expect(receipt.pendingInvitations.map((i) => [i.userId, i.inviteLink, i.reason]))
      .toEqual([
        ["21", "https://t.me/+sharedLink", "background_queue"],
        ["33", "https://t.me/+sharedLink", "background_queue"],
      ])
  })
  test("a fresh chat reads no member before the add and verifies with one listing", async () => {
    const f = fixture()
    withOptional(f)
    expect((await f.run()).state).toBe("ready")
    expect(f.events.filter((e) => e.startsWith("read:"))).toEqual([])
    expect(f.events.filter((e) => e === "list")).toHaveLength(1)
  })
  test("a resumed job reads each required user before adding", async () => {
    const f = fixture()
    withOptional(f)
    f.setup = {
      ...f.setup,
      state: "configuring",
      chatId: -100123,
      binding: { participants: [21, 22, 33], admins: [22], optionalParticipants: [21, 33] },
    }
    f.members.set(22, { role: "member", rights: {} })
    expect((await f.run()).state).toBe("ready")
    expect(f.events).not.toContain("create")
    const order = f.events.filter((e) => /^(read|add):/.test(e))
    expect(order.slice(0, 3)).toEqual([
      "read:22",
      `read:${PROVISIONING_BOT_ID}`,
      `add:${PROVISIONING_BOT_ID}`,
    ])
    expect(order).not.toContain("add:22")
    expect(order).not.toContain("add:21")
  })
  test("an optional member already in the chat is neither queued nor pending", async () => {
    const f = fixture()
    withOptional(f)
    f.members.set(33, { role: "member", rights: {} })
    expect((await f.run()).state).toBe("ready")
    expect(f.queued[0]?.userIds).toEqual([21])
    const receipt = f.setup.receipt as {
      pendingInvitations: { userId: string }[]
      backgroundMembers: { userId: string }[]
    }
    expect(receipt.pendingInvitations.map((i) => i.userId)).toEqual(["21"])
    expect(receipt.backgroundMembers.map((m) => m.userId)).toEqual(["21"])
  })
  test("optional participants without a background queue fail before any write", async () => {
    const f = fixture()
    withOptional(f)
    await expect(
      provisionSetup({
        setup: f.setup,
        cloud: f.cloud,
        journal: f.journal,
        telegram: f.telegram,
        workerId: "worker-one",
      }),
    ).rejects.toThrow("Background member queue required")
    expect(f.events).toEqual([])
  })
  test("a request without optional members writes no background fields", async () => {
    const f = fixture()
    expect((await f.run()).state).toBe("ready")
    expect(f.queued).toEqual([])
    expect(f.setup.receipt).not.toHaveProperty("backgroundMembers")
    expect(f.setup.receipt).not.toHaveProperty("pendingInvitations")
  })
})

test("participant listing keys members by user and skips users who left", () => {
  const roster = provisioningRoster([
    { user: { id: 10 }, status: "creator", isMember: true, permissions: null },
    {
      user: { id: 22 },
      status: "admin",
      isMember: true,
      permissions: { _: "chatAdminRights", inviteUsers: true } as never,
    },
    { user: { id: 21 }, status: "member", isMember: true, permissions: null },
    { user: { id: 40 }, status: "left", isMember: false, permissions: null },
  ])
  expect([...roster.keys()]).toEqual([10, 22, 21])
  expect(roster.get(22)).toEqual({ role: "admin", rights: { inviteUsers: true } })
  expect(roster.get(21)?.role).toBe("member")
})

describe("added but not visible", () => {
  /** `hidden` users accept the add call but never show up as members. */
  function hiding(f: ReturnType<typeof fixture>, hidden: Set<number>) {
    const links: number[] = []
    const sleeps: number[] = []
    f.telegram.add = async (_chat, id) => {
      f.events.push(`add:${id}`)
      if (!hidden.has(id)) f.members.set(id, { role: "member", rights: {} })
    }
    f.telegram.inviteLink = async (_chat, userId) => {
      links.push(userId)
      return {
        inviteLink: "https://t.me/+notVisible",
        expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      }
    }
    const sleep = async (ms: number) => {
      sleeps.push(ms)
    }
    return { links, sleeps, sleep }
  }

  test("a member who shows up on the re-check needs no link", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    const sleep = async (ms: number) => {
      h.sleeps.push(ms)
      f.members.set(22, { role: "member", rights: {} })
    }
    expect((await f.run(sleep)).state).toBe("ready")
    expect(h.sleeps).toEqual([MEMBER_RECHECK_MS])
    expect(h.links).toEqual([])
    expect(f.events.filter((e) => e === "add:22")).toHaveLength(1)
  })

  test("a member still absent gets a link and a retry, not a failure", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    const before = Date.now()
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    const setup = f.setup as Setup & { error: string }
    expect(setup.state).toBe("incomplete")
    expect(setup.error).toBe("verify_added_member:TG_SAI_MEMBER_NOT_VISIBLE")
    const retryMs = Date.parse(setup.retryAt!) - before
    expect(retryMs).toBeGreaterThanOrEqual(MEMBER_NOT_VISIBLE_RETRY_MS)
    expect(retryMs).toBeLessThan(MEMBER_NOT_VISIBLE_RETRY_MS + 60_000)
    expect(h.sleeps).toEqual([MEMBER_RECHECK_MS])
    expect(h.links).toEqual([22])
    expect(
      f.local?.pendingInvitations?.map((i) => [i.userId, i.reason]),
    ).toEqual([["22", NOT_VISIBLE_INVITATION_REASON]])
    expect(f.events).not.toContain("promote:22")
  })

  test("a resumed job waits for the invited member instead of adding again", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    f.events.length = 0

    // Still absent: no add, no new link, still incomplete with a retry.
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
    expect(h.links).toEqual([22])
    expect(f.setup.retryAt).toBeTruthy()

    // Joined by link: promoted and ready without another add.
    f.members.set(22, { role: "member", rights: {} })
    expect((await f.run(h.sleep)).state).toBe("ready")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
    expect(f.events).toContain("promote:22")
    expect(f.setup.receipt).not.toHaveProperty("pendingInvitations")
  })

  test("a resumed job approves the invited member's join request", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    const requests = new Set([22])
    const approvals: number[] = []
    f.telegram.approveJoin = async (_chat, userId) => {
      approvals.push(userId)
      if (!requests.delete(userId)) return false
      f.members.set(userId, { role: "member", rights: {} })
      return true
    }
    f.events.length = 0
    expect((await f.run(h.sleep)).state).toBe("ready")
    expect(approvals).toEqual([22])
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
    expect(f.events).toContain("promote:22")
  })

  test("no waiting join request leaves the member invited", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    f.telegram.approveJoin = async () => false
    f.events.length = 0
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    expect((f.setup as Setup & { error: string }).error).toBe(
      "verify_added_member:TG_SAI_MEMBER_NOT_VISIBLE",
    )
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
  })

  test("an approval error is saved with its own code and retry time", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    f.telegram.approveJoin = async () => {
      throw Object.assign(new Error("flood"), {
        code: 420,
        text: "FLOOD_WAIT_600",
      })
    }
    f.events.length = 0
    const before = Date.now()
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    const setup = f.setup as Setup & { error: string }
    expect(setup.error).toBe("approve_join:FLOOD_WAIT_600")
    expect(Date.parse(setup.retryAt!) - before).toBeGreaterThanOrEqual(600_000)
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])

    f.telegram.approveJoin = async () => {
      throw Object.assign(new Error("x"), { code: 400, text: "CHAT_ADMIN_REQUIRED" })
    }
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    expect((f.setup as Setup & { error: string }).error).toBe(
      "approve_join:CHAT_ADMIN_REQUIRED",
    )
    expect(f.setup.retryAt).toBeNull()
  })

  test("an expired invitation stops for an operator instead of adding again", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    const expire = () => {
      f.local = {
        ...f.local!,
        pendingInvitations: f.local!.pendingInvitations!.map((i) => ({
          ...i,
          expiresAt: new Date(Date.now() - 1000).toISOString(),
        })),
      }
    }
    expire()
    f.events.length = 0
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    const setup = f.setup as Setup & { error: string }
    expect(setup.error).toBe("verify_added_member:TG_SAI_INVITATION_EXPIRED")
    expect(setup.retryAt).toBeNull()
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
    expect(h.links).toEqual([22])

    // An operator resume stays stopped until the person is in.
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
    f.members.set(22, { role: "member", rights: {} })
    expect((await f.run(h.sleep)).state).toBe("ready")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
  })

  test("an expired invitation with a waiting request is approved", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    f.local = {
      ...f.local!,
      pendingInvitations: f.local!.pendingInvitations!.map((i) => ({
        ...i,
        expiresAt: new Date(Date.now() - 1000).toISOString(),
      })),
    }
    f.telegram.approveJoin = async (_chat, userId) => {
      f.members.set(userId, { role: "member", rights: {} })
      return true
    }
    expect((await f.run(h.sleep)).state).toBe("ready")
    expect(f.events.filter((e) => e === "add:22")).toHaveLength(1)
  })

  test("approval goes only to members invited as not visible", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    // Required user 21 left the chat; one case adds a background-queue
    // invitation for them, which must not count as a not-visible one.
    f.members.delete(21)
    const approvals: number[] = []
    f.telegram.approveJoin = async (_chat, userId) => {
      approvals.push(userId)
      return false
    }
    for (const withBackgroundLink of [false, true]) {
      if (withBackgroundLink)
        f.local = {
          ...f.local!,
          pendingInvitations: [
            ...f.local!.pendingInvitations!,
            {
              userId: "21",
              inviteLink: "https://t.me/+shared",
              expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
              reason: "background_queue",
            },
          ],
        }
      f.events.length = 0
      approvals.length = 0
      f.members.delete(21)
      // As if no run had added 21 yet; one that had would send a link.
      f.local = { ...f.local!, addedIds: [] }
      await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
      expect(approvals).toEqual([22])
      expect(f.events.filter((e) => e.startsWith("add:"))).toEqual(["add:21"])
    }
  })

  test("a per-user add failure still saves the not-visible invitation first", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    f.telegram.add = async (_chat, id) => {
      f.events.push(`add:${id}`)
      // A privacy refusal would get its own invitation; this one cannot.
      if (id === 21)
        throw Object.assign(new Error("too many"), {
          code: 400,
          text: "USER_CHANNELS_TOO_MUCH",
        })
      if (id !== 22) f.members.set(id, { role: "member", rights: {} })
    }
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    const setup = f.setup as Setup & { error: string }
    expect(setup.error).toBe("add_member:USER_CHANNELS_TOO_MUCH")
    expect(setup.retryAt).toBeNull()
    expect(
      f.local?.pendingInvitations?.map((i) => [i.userId, i.reason]),
    ).toEqual([["22", NOT_VISIBLE_INVITATION_REASON]])
    // The operator's resume retries 21 but never adds 22 again.
    f.events.length = 0
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual(["add:21"])
  })

  test("a bot that is not visible retries without a link", async () => {
    const f = fixture()
    const hidden = new Set([PROVISIONING_BOT_ID])
    const h = hiding(f, hidden)
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    const setup = f.setup as Setup & { error: string }
    expect(setup.error).toBe(`verify_added_member:${BOT_NOT_VISIBLE}`)
    expect(setup.retryAt).toBeTruthy()
    expect(h.links).toEqual([])
    // The bot shows up later: the resumed job promotes it, no second add.
    f.members.set(PROVISIONING_BOT_ID, { role: "member", rights: {} })
    f.events.length = 0
    expect((await f.run(h.sleep)).state).toBe("ready")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
  })

  test("a bot that never shows is re-added once, then stops for an operator", async () => {
    const f = fixture()
    const h = hiding(f, new Set([PROVISIONING_BOT_ID]))
    const botAdds = () =>
      f.events.filter((e) => e === `add:${PROVISIONING_BOT_ID}`).length
    // Tick 1 adds the bot; tick 2 re-adds it; ticks 3 and 4 add nothing.
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    expect(f.setup.retryAt).toBeTruthy()
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    expect(botAdds()).toBe(MAX_BOT_ADDS)
    expect(f.local?.botAdds).toBe(MAX_BOT_ADDS)
    for (let tick = 0; tick < 2; tick++) {
      await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
      expect((f.setup as Setup & { error: string }).error).toBe(
        `verify_added_member:${BOT_ADDS_EXHAUSTED}`,
      )
      expect(f.setup.retryAt).toBeNull()
    }
    expect(botAdds()).toBe(MAX_BOT_ADDS)
  })

  test("without invitation links the old failure stands", async () => {
    const f = fixture()
    const h = hiding(f, new Set([22]))
    delete f.telegram.inviteLink
    await expect(f.run(h.sleep)).rejects.toThrow("incomplete")
    expect((f.setup as Setup & { error: string }).error).toBe(
      "verify_added_member:provisioning_step_failed",
    )
  })

  test("the not-visible code survives sanitizing", () => {
    expect(safeProvisionError({ text: "TG_SAI_MEMBER_NOT_VISIBLE" })).toBe(
      "TG_SAI_MEMBER_NOT_VISIBLE",
    )
  })
})

describe("transient failure paths", () => {
  const rpc = (code: number, text: string) =>
    tl.RpcError.fromTl({ _: "rpc_error", errorCode: code, errorMessage: text })
  const transientErrors = (): unknown[] => [
    rpc(-503, "Timeout"),
    rpc(500, "RPC_CALL_FAIL"),
    rpc(500, "INTERDC_2_CALL_ERROR"),
    new MtTimeoutError(10_000),
    new TransportError(404),
    new MtcuteError("Connection closed"),
    new MtcuteError("Connection destroyed"),
    Object.assign(new Error("read ECONNRESET"), { code: "ECONNRESET" }),
    new DOMException("The operation was aborted", "AbortError"),
  ]
  const errorOf = (f: ReturnType<typeof fixture>) =>
    (f.setup as Setup & { error?: string }).error
  /** Milliseconds from `before` to the saved retryAt. */
  const retryIn = (f: ReturnType<typeof fixture>, before: number) =>
    Date.parse(f.setup.retryAt!) - before
  function resumed(f: ReturnType<typeof fixture>) {
    f.setup = {
      ...f.setup,
      state: "configuring",
      chatId: -100123,
      binding: { participants: [21, 22], admins: [22] },
    }
  }

  test("timeouts, dropped connections, and server errors are transient", () => {
    for (const error of transientErrors()) {
      expect(isTransientTelegramError(error)).toBe(true)
      expect(safeProvisionError(error)).toBe(TELEGRAM_TRANSIENT)
      expect(provisionRetryAt(error, 0)).toBe(
        new Date(TRANSIENT_RETRY_MS).toISOString(),
      )
    }
    for (const error of [
      rpc(420, "FLOOD_WAIT_30"),
      rpc(403, "USER_PRIVACY_RESTRICTED"),
      rpc(400, "CHAT_ADMIN_REQUIRED"),
      rpc(400, "USER_NOT_PARTICIPANT"),
      new HoldError(Date.now() + 5_000, "invite"),
      new HoldStoreUnavailableError("locked"),
      new Error("network"),
      new Error("Chat marker or custody owner verification failed"),
      null,
    ])
      expect(isTransientTelegramError(error)).toBe(false)
    expect(safeProvisionError(rpc(420, "FLOOD_WAIT_30"))).toBe("FLOOD_WAIT_30")
  })

  test("a transient error at any Telegram step leaves a retryAt", async () => {
    const cases = [
      ["verify_marker", "verifyMarker"],
      ["read_member", "member"],
      ["add_member", "add"],
      ["promote_admin", "promote"],
    ] as const
    for (const error of transientErrors())
      for (const [step, method] of cases) {
        const f = fixture()
        resumed(f)
        f.telegram[method] = async () => {
          throw error
        }
        const before = Date.now()
        await expect(f.run()).rejects.toThrow("incomplete")
        expect(f.setup.state).toBe("incomplete")
        expect(errorOf(f)).toBe(`${step}:${TELEGRAM_TRANSIENT}`)
        expect(retryIn(f, before)).toBeGreaterThanOrEqual(TRANSIENT_RETRY_MS)
        expect(retryIn(f, before)).toBeLessThan(TRANSIENT_RETRY_MS + 60_000)
      }
  })

  test("a transient error on the final listing leaves a retryAt", async () => {
    const f = fixture()
    f.telegram.members = async () => {
      throw new MtTimeoutError(10_000)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`audit_membership:${TELEGRAM_TRANSIENT}`)
    expect(f.setup.retryAt).toBeTruthy()
  })

  test("an add that timed out after it took effect is not repeated", async () => {
    const f = fixture()
    resumed(f)
    const adds: number[] = []
    f.telegram.add = async (_chat, id) => {
      adds.push(id)
      f.members.set(id, { role: "member", rights: {} })
      // The first add takes effect, but its response is lost.
      if (adds.length === 1) throw new MtTimeoutError(10_000)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(adds).toEqual([21])
    expect(f.setup.retryAt).toBeTruthy()
    expect((await f.run()).state).toBe("ready")
    expect(adds).toEqual([21, 22, PROVISIONING_BOT_ID])
  })

  test("a timed-out creation stays unknown, with no retryAt", async () => {
    const f = fixture()
    f.telegram.create = async () => {
      throw new MtTimeoutError(10_000)
    }
    await expect(f.run()).rejects.toThrow("Group creation failed")
    expect(f.setup.state).toBe("creation_unknown")
    expect(f.setup.retryAt).toBeNull()
  })

  test("a creation refused for an unknown session account retries", async () => {
    const f = fixture()
    f.telegram.create = async () => {
      throw new HoldAccountUnknownError("channels.createChannel")
    }
    await expect(f.run()).rejects.toThrow("Group creation failed")
    expect(f.setup.state).toBe("incomplete")
    expect(f.setup.retryAt).toBeTruthy()
    expect(errorOf(f)).toBe("TG_SAI_HOLD_ACCOUNT_UNKNOWN")
  })

  /** Gives the fixture controlled invitation links. */
  function withLinks(f: ReturnType<typeof fixture>) {
    f.telegram.inviteLink = async () => ({
      inviteLink: "https://t.me/+notShown",
      expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
    })
  }
  /** `hidden` users are in the chat but every read and listing omits them. */
  function hide(f: ReturnType<typeof fixture>, hidden: Set<number>) {
    const member = f.telegram.member
    f.telegram.member = async (chat, id) =>
      hidden.has(id) ? { role: "absent", rights: {} } : member(chat, id)
    f.telegram.members = async () => {
      const listed = new Map(f.members)
      for (const id of hidden) listed.delete(id)
      return listed
    }
  }

  test("promoting a user not shown in the chat yet retries", async () => {
    for (const bot of [false, true]) {
      const f = fixture()
      withLinks(f)
      const promote = f.telegram.promote
      f.telegram.promote = async (chat, id, rights) => {
        if ((id === PROVISIONING_BOT_ID) === bot)
          throw rpc(400, "USER_NOT_PARTICIPANT")
        await promote(chat, id, rights)
      }
      await expect(f.run()).rejects.toThrow("incomplete")
      expect(errorOf(f)).toBe(
        bot
          ? `promote_bot:${BOT_NOT_VISIBLE}`
          : `promote_admin:${MEMBER_NOT_VISIBLE}`,
      )
      expect(f.setup.retryAt).toBeTruthy()
      // The resumed job finds the member and promotes without an add.
      f.telegram.promote = promote
      f.events.length = 0
      expect((await f.run()).state).toBe("ready")
      expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
    }
  })

  test("a person refused promotion as absent gets a link, never a re-add", async () => {
    const f = fixture()
    withLinks(f)
    const promote = f.telegram.promote
    f.telegram.promote = async (chat, id, rights) => {
      if (id === 22) throw rpc(400, "USER_NOT_PARTICIPANT")
      await promote(chat, id, rights)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(
      f.local?.pendingInvitations?.map((i) => [i.userId, i.reason]),
    ).toEqual([["22", NOT_VISIBLE_INVITATION_REASON]])
    // Still hidden on every resume: the job waits on the link, no add.
    hide(f, new Set([22]))
    f.events.length = 0
    for (let tick = 0; tick < 3; tick++)
      await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
    expect(errorOf(f)).toBe(`verify_added_member:${MEMBER_NOT_VISIBLE}`)
  })

  test("a required member missing from the final listing retries", async () => {
    const f = fixture()
    withLinks(f)
    // Added, but the listing right after does not show them yet.
    f.telegram.members = async () => {
      const listed = new Map(f.members)
      listed.delete(21)
      return listed
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`audit_membership:${MEMBER_NOT_VISIBLE}`)
    expect(f.setup.retryAt).toBeTruthy()
    f.telegram.members = async () => new Map(f.members)
    f.events.length = 0
    expect((await f.run()).state).toBe("ready")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
  })

  test("a member who stays missing from the listing is never re-added", async () => {
    const f = fixture()
    withLinks(f)
    hide(f, new Set([21]))
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`audit_membership:${MEMBER_NOT_VISIBLE}`)
    expect(
      f.local?.pendingInvitations?.map((i) => [i.userId, i.reason]),
    ).toEqual([["21", NOT_VISIBLE_INVITATION_REASON]])
    f.events.length = 0
    for (let tick = 0; tick < 3; tick++)
      await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
  })

  test("a bot missing from the listing is re-added once, then stops", async () => {
    const f = fixture()
    hide(f, new Set([PROVISIONING_BOT_ID]))
    const botAdds = () =>
      f.events.filter((e) => e === `add:${PROVISIONING_BOT_ID}`).length
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`audit_membership:${BOT_NOT_VISIBLE}`)
    for (let tick = 0; tick < 3; tick++)
      await expect(f.run()).rejects.toThrow("incomplete")
    expect(botAdds()).toBe(MAX_BOT_ADDS)
    expect(errorOf(f)).toBe(`verify_added_member:${BOT_ADDS_EXHAUSTED}`)
    expect(f.setup.retryAt).toBeNull()
  })

  test("transient add failures in consecutive runs stop after the limit", async () => {
    const f = fixture()
    withLinks(f)
    resumed(f)
    const adds: number[] = []
    f.telegram.add = async (_chat, id) => {
      adds.push(id)
      throw rpc(-503, "Timeout")
    }
    // Each run reaches a new add: 21, then 22, then the bot.
    for (let tick = 1; tick <= MAX_TRANSIENT_RETRIES + 2; tick++) {
      if (tick > 1 && !f.setup.retryAt) break
      await expect(f.run()).rejects.toThrow("incomplete")
    }
    expect(adds).toEqual([21, 22, PROVISIONING_BOT_ID])
    expect(errorOf(f)).toBe(`add_member:${TRANSIENT_LIMIT}`)
    expect(f.setup.retryAt).toBeNull()
    expect(f.local?.transientRetries).toBe(MAX_TRANSIENT_RETRIES)
  })

  test("reads that time out never count toward the limit", async () => {
    const f = fixture()
    resumed(f)
    let reads = 0
    f.telegram.verifyMarker = async () => {
      reads++
      throw rpc(-503, "Timeout")
    }
    for (let tick = 0; tick < MAX_TRANSIENT_RETRIES + 2; tick++)
      await expect(f.run()).rejects.toThrow("incomplete")
    expect(reads).toBe(MAX_TRANSIENT_RETRIES + 2)
    expect(errorOf(f)).toBe(`verify_marker:${TELEGRAM_TRANSIENT}`)
    expect(f.setup.retryAt).toBeTruthy()
  })

  test("a privacy wait over 24 h with read timeouts never hits the limit", async () => {
    const f = fixture()
    withLinks(f)
    f.telegram.add = async (_chat, id) => {
      if (id === 21) throw rpc(403, "USER_PRIVACY_RESTRICTED")
      f.members.set(id, { role: "member", rights: {} })
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    const member = f.telegram.member
    let reads = 0
    f.telegram.member = async (chat, id) => {
      // About one read in 30 times out.
      if (++reads % 30 === 0) throw new MtTimeoutError(10_000)
      return member(chat, id)
    }
    // A resume every 15 minutes for a day.
    for (let tick = 0; tick < 96; tick++) {
      await expect(f.run()).rejects.toThrow("incomplete")
      expect(f.setup.retryAt).toBeTruthy()
      expect(errorOf(f)).not.toContain(TRANSIENT_LIMIT)
    }
  })

  test("runs cut short by worker shutdown never hit the limit", async () => {
    const f = fixture()
    resumed(f)
    const controller = new AbortController()
    f.telegram.add = async () => {
      controller.abort()
      throw new MtcuteError("Client is destroyed")
    }
    for (let tick = 0; tick < MAX_TRANSIENT_RETRIES + 1; tick++)
      await expect(
        provisionSetup({
          setup: f.setup,
          cloud: f.cloud,
          journal: f.journal,
          telegram: f.telegram,
          workerId: "worker-one",
          background: f.background,
          signal: controller.signal,
        }),
      ).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`add_member:${TELEGRAM_TRANSIENT}`)
    expect(f.setup.retryAt).toBeTruthy()
    expect(f.local?.transientRetries ?? 0).toBe(0)
  })

  test("a run that gets past the adds resets the transient count", async () => {
    const f = fixture()
    withLinks(f)
    resumed(f)
    const add = f.telegram.add
    let timeouts = 1
    f.telegram.add = async (chat, id) => {
      if (timeouts-- > 0) throw rpc(-503, "Timeout")
      await add(chat, id)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.local?.transientRetries).toBe(1)
    // 21 now waits on a link; the others are added, so the count resets.
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.local?.transientRetries ?? 0).toBe(0)
  })

  test("a failed journal write before an add keeps no phantom counters", async () => {
    const f = fixture()
    resumed(f)
    let sent = 0
    f.telegram.add = async () => {
      sent++
    }
    const write = f.journal.write
    let failed = false
    f.journal.write = async (id, record) => {
      if (!failed && (record.addedIds?.length || record.botAdds)) {
        failed = true
        throw new Error("disk full")
      }
      await write(id, record)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(sent).toBe(0)
    expect(f.local?.addedIds).toBeUndefined()
    expect(f.local?.botAdds).toBeUndefined()
  })

  test("a failed rollback write keeps the flood's code and retry time", async () => {
    const f = fixture()
    f.telegram.addMany = async () => {
      throw rpc(420, "FLOOD_WAIT_60")
    }
    const write = f.journal.write
    let journaled = false
    f.journal.write = async (id, record) => {
      // The pre-add write works; the take-back write fails.
      if (record.addedIds?.length) journaled = true
      else if (journaled && record.state === "configuring")
        throw new Error("disk full")
      await write(id, record)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe("add_member:FLOOD_WAIT_60")
    expect(f.setup.retryAt).toBeTruthy()
  })

  test("an add that timed out is never sent again; the person gets a link", async () => {
    const f = fixture()
    withLinks(f)
    resumed(f)
    const adds: number[] = []
    const add = f.telegram.add
    f.telegram.add = async (chat, id) => {
      adds.push(id)
      if (id === 21) throw rpc(-503, "Timeout")
      await add(chat, id)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`add_member:${TELEGRAM_TRANSIENT}`)
    for (let tick = 0; tick < 3; tick++)
      await expect(f.run()).rejects.toThrow("incomplete")
    expect(adds.filter((id) => id === 21)).toHaveLength(1)
    expect(errorOf(f)).toBe(`verify_added_member:${MEMBER_NOT_VISIBLE}`)
    expect(
      f.local?.pendingInvitations?.map((i) => [i.userId, i.reason]),
    ).toEqual([["21", NOT_VISIBLE_INVITATION_REASON]])
  })

  test("an add cut off by an unknown error is journaled before it is sent", async () => {
    const f = fixture()
    withLinks(f)
    resumed(f)
    const adds: number[] = []
    const add = f.telegram.add
    f.telegram.add = async (chat, id) => {
      adds.push(id)
      // Like a crash after Telegram accepted the call: no answer arrives.
      if (id === 21) throw new Error("worker died mid-call")
      await add(chat, id)
    }
    expect(f.local).toBeNull()
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.local?.addedIds).toEqual([21])
    for (let tick = 0; tick < 2; tick++)
      await expect(f.run()).rejects.toThrow("incomplete")
    expect(adds.filter((id) => id === 21)).toHaveLength(1)
  })

  test("a hidden person is not re-added when the stop is for someone else", async () => {
    const f = fixture()
    withLinks(f)
    hide(f, new Set([21, 22]))
    const promote = f.telegram.promote
    f.telegram.promote = async (chat, id, rights) => {
      if (id === 22) throw rpc(400, "USER_NOT_PARTICIPANT")
      await promote(chat, id, rights)
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`promote_admin:${MEMBER_NOT_VISIBLE}`)
    // A link wait: the background drain may run meanwhile.
    expect(waitingOnInvitedMember(f.setup)).toBe(true)
    f.events.length = 0
    for (let tick = 0; tick < 3; tick++)
      await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
  })

  test("a failed link for an unlisted person never leads to a re-add", async () => {
    const f = fixture()
    hide(f, new Set([21]))
    let failLink = true
    f.telegram.inviteLink = async () => {
      if (failLink) throw rpc(-503, "Timeout")
      return {
        inviteLink: "https://t.me/+notShown",
        expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      }
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`create_invitation:${TELEGRAM_TRANSIENT}`)
    failLink = false
    f.events.length = 0
    for (let tick = 0; tick < 2; tick++)
      await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.events.filter((e) => e.startsWith("add:"))).toEqual([])
    expect(errorOf(f)).toBe(`verify_added_member:${MEMBER_NOT_VISIBLE}`)
  })

  test("a refused batch does not count as a bot add", async () => {
    // Two floods on the whole batch, then a batch that adds everyone.
    const f = fixture()
    let floods = 2
    f.telegram.addMany = async (_chat, ids) => {
      if (floods-- > 0) throw rpc(420, "FLOOD_WAIT_60")
      for (const id of ids) {
        f.events.push(`add:${id}`)
        f.members.set(id, { role: "member", rights: {} })
      }
      return []
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(f.local?.botAdds ?? 0).toBe(0)
    expect((await f.run()).state).toBe("ready")
    expect(f.local?.botAdds).toBe(1)
  })

  test("a person's error ahead of the bot does not count as a bot add", async () => {
    const f = fixture()
    resumed(f)
    f.telegram.add = async () => {
      throw rpc(-503, "Timeout")
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    await expect(f.run()).rejects.toThrow("incomplete")
    // 21 fails first each time, so no call ever reached the bot.
    expect(f.local?.botAdds ?? 0).toBe(0)
  })

  test("only the bot unseen is not an invitation wait", async () => {
    const f = fixture()
    const hidden = new Set([PROVISIONING_BOT_ID])
    f.telegram.add = async (_chat, id) => {
      f.events.push(`add:${id}`)
      if (!hidden.has(id)) f.members.set(id, { role: "member", rights: {} })
    }
    await expect(f.run(async () => {})).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`verify_added_member:${BOT_NOT_VISIBLE}`)
    expect(waitingOnInvitedMember(f.setup)).toBe(false)
  })

  test("a wrong owner or missing rights still needs an operator", async () => {
    const owner = fixture()
    owner.telegram.members = async () => {
      const listed = new Map(owner.members)
      listed.set(10, { role: "admin", rights: {} })
      listed.delete(21)
      return listed
    }
    await expect(owner.run()).rejects.toThrow("incomplete")
    expect(errorOf(owner)).toBe("audit_membership:provisioning_step_failed")
    expect(owner.setup.retryAt).toBeNull()

    const rights = fixture()
    rights.telegram.promote = async () => {}
    await expect(rights.run()).rejects.toThrow("incomplete")
    expect(errorOf(rights)).toBe("audit_membership:provisioning_step_failed")
    expect(rights.setup.retryAt).toBeNull()
  })

  test("a failed background queue write retries", async () => {
    const f = fixture()
    f.setup = { ...f.setup, optionalParticipants: ["alice"] }
    f.telegram.members = async () => {
      const listed = new Map(f.members)
      listed.delete(21)
      return listed
    }
    f.telegram.add = async (_chat, id) => {
      if (id !== 21) f.members.set(id, { role: "member", rights: {} })
    }
    const queue: BackgroundEnqueue = {
      enqueue: () => {
        throw new Error("database is locked")
      },
    }
    await expect(
      provisionSetup({
        setup: f.setup,
        cloud: f.cloud,
        journal: f.journal,
        telegram: f.telegram,
        workerId: "worker-one",
        background: queue,
      }),
    ).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`enqueue_background:${QUEUE_UNAVAILABLE}`)
    expect(f.setup.retryAt).toBeTruthy()
  })

  test("an expired privacy invitation stops for an operator, no add", async () => {
    const f = fixture()
    f.telegram.inviteLink = async () => ({
      inviteLink: "https://t.me/+privacy",
      expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
    })
    const adds: number[] = []
    f.telegram.add = async (_chat, id) => {
      adds.push(id)
      if (id === 21) throw rpc(403, "USER_PRIVACY_RESTRICTED")
      f.members.set(id, { role: "member", rights: {} })
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(waitingOnInvitedMember(f.setup)).toBe(true)
    f.local = {
      ...f.local!,
      pendingInvitations: f.local!.pendingInvitations!.map((i) => ({
        ...i,
        expiresAt: new Date(Date.now() - 1000).toISOString(),
      })),
    }
    adds.length = 0
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe("verify_added_member:TG_SAI_INVITATION_EXPIRED")
    expect(f.setup.retryAt).toBeNull()
    expect(adds).toEqual([])
  })

  test("a privacy-invited member is approved on resume, never added again", async () => {
    const f = fixture()
    f.telegram.inviteLink = async () => ({
      inviteLink: "https://t.me/+privacy",
      expiresAt: new Date(Date.now() + 86_400_000).toISOString(),
    })
    let adds = 0
    f.telegram.add = async (_chat, id) => {
      if (id === 21) {
        adds++
        throw rpc(403, "USER_PRIVACY_RESTRICTED")
      }
      f.events.push(`add:${id}`)
      f.members.set(id, { role: "member", rights: {} })
    }
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(errorOf(f)).toBe(`add_member:${AWAITING_JOIN}`)
    const before = Date.now()
    await expect(f.run()).rejects.toThrow("incomplete")
    expect(retryIn(f, before)).toBeGreaterThanOrEqual(
      MEMBER_NOT_VISIBLE_RETRY_MS,
    )
    expect(adds).toBe(1)
    f.telegram.approveJoin = async (_chat, userId) => {
      f.members.set(userId, { role: "member", rights: {} })
      return true
    }
    expect((await f.run()).state).toBe("ready")
    expect(adds).toBe(1)
  })
})
