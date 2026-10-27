import { describe, expect, test } from "bun:test"
import { mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { addProfile, strictProfileCredentials } from "./profiles"
import {
  BOT_BASELINE_RIGHTS,
  creationMarker,
  PROVISIONING_BOT_ID,
  provisionSetup,
  requestFingerprint,
  safeProvisionError,
  TEAM_ADMIN_RIGHTS,
  validateRequest,
} from "./provisioning"
import type {
  JournalRecord,
  Member,
  Progress,
  ProvisionCloud,
  ProvisionJournal,
  ProvisionRequest,
  Setup,
  TelegramProvisioner,
} from "./provisioning"
import { CliProvisionCloud, cloudRequest } from "./provisioningCloud"
import {
  accountPaths,
  FileProvisionJournal,
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
      members.set(id, { role: "member", rights: {} })
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
    run: () =>
      provisionSetup({
        setup,
        cloud,
        journal,
        telegram,
        workerId: "worker-one",
      }),
  }
}

describe("custody provisioning", () => {
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
      if (id === 22) throw new Error("privacy")
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
    expect((f.setup as Setup & { error: string }).error).toBe("FLOOD_WAIT_40")
    expect(safeProvisionError(new Error("session string"))).toBe(
      "provisioning_step_failed",
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
    expect(calls[0]?.input.admins).toEqual([
      { userId: "22", rightsProfile: "team" },
    ])
    expect(result.binding).toEqual({ participants: [21, 22], admins: [22] })
    await cloud.progress(requestValue.requestId, "token", {
      state: "creating",
      binding: result.binding,
    })
    expect(calls[1]?.input.resolvedParticipants).toEqual(["21", "22"])
    expect(calls[1]?.input).not.toHaveProperty("binding")
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
  })
})
