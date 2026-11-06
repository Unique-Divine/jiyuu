import { describe, expect, test } from "bun:test"
import { Long } from "@mtcute/bun"

import {
  collectIds,
  isNameOnlyMethod,
  summarizeRequest,
  summarizeTl,
  type Summary,
} from "./summarize"

// Distinct values that must never reach a summary. Each is long enough that
// an accidental substring match is impossible.
const SECRETS = {
  password: "hunter2-cloud-password-value",
  phone: "+15550001234",
  phoneCode: "73915",
  phoneCodeHash: "f00dfacecafe1234beef",
  session: "1BVtsOKcBu2session-string-AAAA",
  apiHash: "0123456789abcdef0123456789abcdef",
  accessHash: "-4823982301928374651",
  messageText: "Welcome to the private Sai deal room for Acme",
  username: "acme_ceo_handle",
  title: "Sai x Acme OTC Deal",
  about: "Confidential OTC desk for Acme Treasury",
  token: "7012345678:AAHsecret-bot-token-value",
}

const SRP_A = new Uint8Array(256).fill(0xa1)
const SRP_M1 = new Uint8Array(32).fill(0xb2)

function inputUser(userId: number): Record<string, unknown> {
  return {
    _: "inputUser",
    userId,
    accessHash: Long.fromString(SECRETS.accessHash),
  }
}

function inputChannel(channelId: number): Record<string, unknown> {
  return {
    _: "inputChannel",
    channelId,
    accessHash: Long.fromString(SECRETS.accessHash),
  }
}

interface Case {
  name: string
  method: string
  request: unknown
  expected: Summary
}

const CASES: Case[] = [
  {
    name: "inviteToChannel keeps channel and user ids",
    method: "channels.inviteToChannel",
    request: {
      _: "channels.inviteToChannel",
      channel: inputChannel(2_345_678_901),
      users: [inputUser(111), inputUser(222), inputUser(333)],
    },
    expected: {
      _: "channels.inviteToChannel",
      channel: { _: "inputChannel", channelId: 2_345_678_901 },
      users: { n: 3, ids: [111, 222, 333], of: "inputUser" },
    },
  },
  {
    name: "createChannel keeps flags, measures title and about",
    method: "channels.createChannel",
    request: {
      _: "channels.createChannel",
      broadcast: false,
      megagroup: true,
      forum: false,
      title: SECRETS.title,
      about: SECRETS.about,
      ttlPeriod: 0,
    },
    expected: {
      _: "channels.createChannel",
      broadcast: false,
      megagroup: true,
      forum: false,
      title: { len: SECRETS.title.length },
      about: { len: SECRETS.about.length },
      ttlPeriod: 0,
    },
  },
  {
    name: "sendMessage drops message text and peer access hash",
    method: "messages.sendMessage",
    request: {
      _: "messages.sendMessage",
      noWebpage: true,
      peer: {
        _: "inputPeerChannel",
        channelId: 2_345_678_901,
        accessHash: Long.fromString(SECRETS.accessHash),
      },
      message: SECRETS.messageText,
      randomId: Long.fromString("998877665544332211"),
      entities: [
        { _: "messageEntityBold", offset: 0, length: 7 },
        { _: "messageEntityMentionName", offset: 8, length: 4, userId: 444 },
      ],
    },
    expected: {
      _: "messages.sendMessage",
      noWebpage: true,
      peer: { _: "inputPeerChannel", channelId: 2_345_678_901 },
      message: { len: SECRETS.messageText.length },
      randomId: "998877665544332211",
      entities: { n: 2, ids: [444], of: "messageEntityBold" },
    },
  },
  {
    name: "resolveUsername measures the username",
    method: "contacts.resolveUsername",
    request: { _: "contacts.resolveUsername", username: SECRETS.username },
    expected: {
      _: "contacts.resolveUsername",
      username: { len: SECRETS.username.length },
    },
  },
  {
    name: "editCreator keeps ids and never the SRP password",
    method: "channels.editCreator",
    request: {
      _: "channels.editCreator",
      channel: inputChannel(2_345_678_901),
      userId: inputUser(555),
      password: {
        _: "inputCheckPasswordSRP",
        srpId: Long.fromString("1234567890123"),
        A: SRP_A,
        M1: SRP_M1,
      },
    },
    expected: {
      _: "channels.editCreator",
      channel: { _: "inputChannel", channelId: 2_345_678_901 },
      userId: { _: "inputUser", userId: 555 },
    },
  },
  {
    name: "auth.signIn records only the method",
    method: "auth.signIn",
    request: {
      _: "auth.signIn",
      phoneNumber: SECRETS.phone,
      phoneCodeHash: SECRETS.phoneCodeHash,
      phoneCode: SECRETS.phoneCode,
    },
    expected: { _: "auth.signIn" },
  },
  {
    name: "auth.sendCode records only the method",
    method: "auth.sendCode",
    request: {
      _: "auth.sendCode",
      phoneNumber: SECRETS.phone,
      apiId: 12345,
      apiHash: SECRETS.apiHash,
      settings: { _: "codeSettings" },
    },
    expected: { _: "auth.sendCode" },
  },
  {
    name: "account.checkPassword records only the method",
    method: "account.checkPassword",
    request: {
      _: "account.checkPassword",
      password: { _: "inputCheckPasswordSRP", A: SRP_A, M1: SRP_M1 },
    },
    expected: { _: "account.checkPassword" },
  },
  {
    name: "wrapped initConnection drops apiHash, session, and token keys",
    method: "invokeWithLayer",
    request: {
      _: "invokeWithLayer",
      layer: 200,
      query: {
        _: "initConnection",
        apiId: 12345,
        apiHash: SECRETS.apiHash,
        session: SECRETS.session,
        botToken: SECRETS.token,
      },
    },
    expected: {
      _: "invokeWithLayer",
      layer: 200,
      query: { _: "initConnection", apiId: 12345 },
    },
  },
]

const RESPONSE = {
  _: "users.userFull",
  fullUser: { _: "userFull", id: 777, about: SECRETS.about },
  users: [
    {
      _: "user",
      id: 777,
      accessHash: Long.fromString(SECRETS.accessHash),
      firstName: "Acme",
      username: SECRETS.username,
      phone: SECRETS.phone,
      bot: false,
    },
  ],
  chats: [],
}

function allSummaries(): Summary[] {
  return [
    ...CASES.map((c) => summarizeRequest(c.method, c.request)),
    ...CASES.map((c) => summarizeTl(c.request)),
    summarizeTl(RESPONSE),
    summarizeTl({ _: "updateShort", session: SECRETS.session }),
  ]
}

describe("summarizeRequest", () => {
  for (const c of CASES) {
    test(c.name, () => {
      expect(summarizeRequest(c.method, c.request)).toEqual(c.expected)
    })
  }

  test("isNameOnlyMethod covers auth and password methods", () => {
    expect(isNameOnlyMethod("auth.importAuthorization")).toBe(true)
    expect(isNameOnlyMethod("account.getPassword")).toBe(true)
    expect(isNameOnlyMethod("channels.editCreator")).toBe(false)
  })
})

describe("secrets never reach a summary", () => {
  const json = allSummaries().map((s) => JSON.stringify(s)).join("\n")

  for (const [name, secret] of Object.entries(SECRETS)) {
    test(`${name} value is absent`, () => {
      expect(json).not.toContain(secret)
    })
  }

  test("secret-bearing keys are absent", () => {
    for (const key of [
      "accessHash",
      "access_hash",
      "password",
      "phone",
      "phoneNumber",
      "phoneCodeHash",
      "apiHash",
      "session",
      "botToken",
      "srpId",
      '"A"',
      '"M1"',
    ]) {
      expect(json).not.toContain(key)
    }
  })

  test("SRP byte buffers leave no trace", () => {
    expect(json).not.toContain("161")
    expect(json).not.toContain('"len":256')
  })
})

describe("summarizeTl", () => {
  test("primitives, bytes, and nullish values", () => {
    expect(summarizeTl(42)).toBe(42)
    expect(summarizeTl(true)).toBe(true)
    expect(summarizeTl(null)).toBeNull()
    expect(summarizeTl(undefined)).toBeNull()
    expect(summarizeTl(Number.NaN)).toBeNull()
    expect(summarizeTl(10n)).toBe("10")
    expect(summarizeTl("abc")).toEqual({ len: 3 })
    expect(summarizeTl(new Uint8Array(9))).toEqual({ len: 9 })
    expect(summarizeTl(Buffer.from("hello"))).toEqual({ len: 5 })
  })

  test("response summary keeps ids and drops contact details", () => {
    expect(summarizeTl(RESPONSE)).toEqual({
      _: "users.userFull",
      fullUser: {
        _: "userFull",
        id: 777,
        about: { len: SECRETS.about.length },
      },
      users: { n: 1, ids: [777], of: "user" },
      chats: { n: 0 },
    })
  })

  test("arrays report at most five ids", () => {
    const users = [1, 2, 3, 4, 5, 6, 7].map((id) => inputUser(id))
    expect(summarizeTl(users)).toEqual({
      n: 7,
      ids: [1, 2, 3, 4, 5],
      of: "inputUser",
    })
    expect(collectIds([{ _: "x", peer: { chatId: 9 } }, 12])).toEqual([9, 12])
  })

  test("depth is capped at three levels", () => {
    const deep = {
      _: "l0",
      a: { _: "l1", b: { _: "l2", c: { _: "l3", d: { _: "l4", n: 1 } } } },
      plain: { x: { y: { z: { w: 1 } } } },
    }
    expect(summarizeTl(deep)).toEqual({
      _: "l0",
      a: { _: "l1", b: { _: "l2", c: { _: "l3" } } },
      plain: { x: { y: { truncated: "depth" } } },
    })
  })

  test("oversized output is replaced by a valid JSON marker", () => {
    const wide: Record<string, unknown> = { _: "messages.huge" }
    for (let i = 0; i < 500; i += 1) wide[`field${i}`] = i
    const summary = summarizeTl(wide, { maxBytes: 2048 })
    const json = JSON.stringify(summary)
    expect(json.length).toBeLessThanOrEqual(2048)
    expect(summary).toMatchObject({ _: "messages.huge", truncated: "size" })
    expect(() => JSON.parse(json)).not.toThrow()
  })

  test("oversized nesting falls back to a shallower depth", () => {
    const nested: Record<string, unknown> = { _: "root" }
    for (let i = 0; i < 100; i += 1) {
      nested[`k${i}`] = { _: "child", inner: { a: 1, b: 2, c: 3 } }
    }
    const summary = summarizeTl(nested, { maxBytes: 2048 }) as Record<
      string,
      Summary
    >
    expect(JSON.stringify(summary).length).toBeLessThanOrEqual(2048)
  })
})
