import { describe, expect, test } from "bun:test"
import {
  CHAT_DESCRIPTION_TEXT,
  chatDescription,
  creationMarker,
  descriptionMatchesMarker,
  requestFingerprint,
  setupRef,
} from "./provisioning"
import type {
  ChatInfo,
  JournalRecord,
  ProvisionRequest,
  Setup,
  TelegramProvisioner,
} from "./provisioning"
import { rewriteChatDescription } from "./provisioningDescription"
import { markerCandidates, markerChatMatches } from "./provisioningRuntime"

function request(requestId = "describe-setup-1"): ProvisionRequest {
  return {
    requestId,
    custodyProfile: "custody",
    expectedUserId: 10,
    title: "Sai describe",
    participants: [21],
    admins: [{ userId: 22, rightsProfile: "team" }],
    botRightsProfile: "baseline",
    dealIds: ["deal-a"],
  }
}
const CHAT_ID = -1001234567890
function readySetup(overrides: Partial<Setup> = {}): Setup {
  return { ...request(), state: "ready", chatId: CHAT_ID, ...overrides }
}
/** A ref with every hex digit changed, so it never equals the real one. */
function otherRef(ref: string): string {
  return [...ref]
    .map((c) => ((parseInt(c, 16) + 1) % 16).toString(16))
    .join("")
}

/** Telegram state for one chat, with the adapter methods the rewrite uses. */
function chatStore(chat: Partial<ChatInfo> = {}) {
  const state: ChatInfo = {
    title: "Sai describe",
    description: creationMarker(request()),
    chatType: "supergroup",
    isForum: false,
    isCreator: true,
    ...chat,
  }
  const writes: { chatId: number; description: string }[] = []
  const telegram: Pick<
    TelegramProvisioner,
    "self" | "chatInfo" | "setDescription"
  > = {
    self: async () => 10,
    chatInfo: async (chatId) => {
      expect(chatId).toBe(CHAT_ID)
      return { ...state }
    },
    setDescription: async (chatId, description) => {
      writes.push({ chatId, description })
      state.description = description
    },
  }
  return { state, writes, telegram }
}

describe("chat description", () => {
  test("new description is the friendly text and the first 8 hex of the fingerprint", () => {
    const ref = requestFingerprint(request()).slice(0, 8)
    expect(setupRef(request())).toBe(ref)
    expect(ref).toMatch(/^[0-9a-f]{8}$/)
    expect(chatDescription(request())).toBe(
      `Chat with the Sai team. Ref: ${ref}`,
    )
    expect(chatDescription(request())).toBe(
      `${CHAT_DESCRIPTION_TEXT} Ref: ${ref}`,
    )
    expect(chatDescription(request())).not.toContain("sai-setup:")
    expect(chatDescription(request())).not.toContain("deal-a")
  })

  test("legacy marker and new description both match; other text does not", () => {
    const marker = creationMarker(request())
    const ref = setupRef(request())
    expect(descriptionMatchesMarker(marker, marker)).toBe(true)
    expect(descriptionMatchesMarker(chatDescription(request()), marker)).toBe(
      true,
    )
    // The wording may change; only the trailing ref is read.
    expect(descriptionMatchesMarker(`New wording. Ref: ${ref}`, marker)).toBe(
      true,
    )
    for (const description of [
      `Chat with the Sai team. Ref: ${otherRef(ref)}`,
      chatDescription(request("describe-setup-2")),
      creationMarker(request("describe-setup-2")),
      `Chat with the Sai team. Ref: ${ref} `,
      `Chat with the Sai team. Ref: ${ref.toUpperCase()}`,
      `Chat with the Sai team. Ref: ${ref}0`,
      `Chat with the Sai team. Ref:${ref}`,
      `Chat with the Sai team.Ref: ${ref}`,
      `Ref: ${ref} trailing`,
      marker.slice(0, -1),
      "",
    ])
      expect(descriptionMatchesMarker(description, marker)).toBe(false)
    // A malformed expected marker matches only itself, as before.
    expect(descriptionMatchesMarker(`Ref: ${ref}`, `sai-setup:${ref}`)).toBe(
      false,
    )
  })

  test("marker check still requires a creator-owned non-forum supergroup", () => {
    const marker = creationMarker(request())
    const base = {
      chatType: "supergroup",
      isForum: false,
      isCreator: true,
    }
    expect(markerChatMatches({ ...base, bio: marker }, marker)).toBe(true)
    expect(
      markerChatMatches({ ...base, bio: chatDescription(request()) }, marker),
    ).toBe(true)
    expect(
      markerChatMatches(
        { ...base, bio: `Ref: ${otherRef(setupRef(request()))}` },
        marker,
      ),
    ).toBe(false)
    for (const bio of [marker, chatDescription(request())]) {
      expect(markerChatMatches({ ...base, isCreator: false, bio }, marker)).toBe(
        false,
      )
      expect(markerChatMatches({ ...base, isForum: true, bio }, marker)).toBe(
        false,
      )
      const group = { ...base, chatType: "group", bio }
      expect(markerChatMatches(group, marker)).toBe(false)
    }
  })

  test("candidates find chats with either form and skip the rest", async () => {
    const marker = creationMarker(request())
    const own = { chatType: "supergroup", isForum: false, isCreator: true }
    const chats = new Map<number, typeof own & { bio: string }>([
      [-1001, { ...own, bio: marker }],
      [-1002, { ...own, bio: chatDescription(request()) }],
      [-1003, { ...own, bio: chatDescription(request("describe-setup-2")) }],
      [-1004, { ...own, isCreator: false, bio: marker }],
      [-1005, { ...own, bio: "Some other group" }],
    ])
    const peers = [
      { type: "user", id: 77 },
      ...[...chats].map(([id, chat]) => ({
        type: "chat",
        id,
        chatType: chat.chatType,
        isCreator: chat.isCreator,
      })),
    ]
    async function* dialogs() {
      for (const peer of peers) yield { peer }
    }
    const read: number[] = []
    const found = await markerCandidates(dialogs(), async (chatId) => {
      read.push(chatId)
      return markerChatMatches(chats.get(chatId)!, marker)
    })
    expect(found).toEqual([-1001, -1002])
    // Chats the custody account did not create are never read in full.
    expect(read).toEqual([-1001, -1002, -1003, -1005])
  })
})

describe("rewrite-description", () => {
  test("dry run prints title, current and new description without writing", async () => {
    const store = chatStore()
    const result = await rewriteChatDescription({
      setup: readySetup(),
      local: null,
      telegram: store.telegram,
      run: false,
    })
    expect(result).toEqual({
      dryRun: true,
      requestId: "describe-setup-1",
      chatId: CHAT_ID,
      title: "Sai describe",
      currentDescription: creationMarker(request()),
      newDescription: chatDescription(request()),
      changed: true,
    })
    expect(store.writes).toEqual([])
    expect(store.state.description).toBe(creationMarker(request()))
  })

  test("--run writes the new description once and reads it back", async () => {
    const store = chatStore()
    const result = await rewriteChatDescription({
      setup: readySetup(),
      local: null,
      telegram: store.telegram,
      run: true,
    })
    expect(result.dryRun).toBe(false)
    expect(result.changed).toBe(true)
    expect(store.writes).toEqual([
      { chatId: CHAT_ID, description: chatDescription(request()) },
    ])
    // The rewritten chat still verifies for this setup.
    expect(
      markerChatMatches(
        { ...store.state, bio: store.state.description },
        creationMarker(request()),
      ),
    ).toBe(true)
    // A second run finds nothing to change.
    const again = await rewriteChatDescription({
      setup: readySetup(),
      local: null,
      telegram: store.telegram,
      run: true,
    })
    expect(again.changed).toBe(false)
    expect(store.writes).toHaveLength(1)
  })

  test("uses the local receipt's chat when the setup has none", async () => {
    const store = chatStore()
    const local: JournalRecord = {
      fingerprint: requestFingerprint(request()),
      state: "configuring",
      chatId: CHAT_ID,
    }
    const result = await rewriteChatDescription({
      setup: readySetup({ state: "creation_unknown", chatId: undefined }),
      local,
      telegram: store.telegram,
      run: false,
    })
    expect(result.chatId).toBe(CHAT_ID)
    expect(result.changed).toBe(true)
  })

  const refusals: {
    name: string
    error: string
    chat?: Partial<ChatInfo>
    setup?: Partial<Setup>
    local?: JournalRecord
    self?: number
  }[] = [
    {
      name: "a hand-edited description",
      error: "not this setup's legacy marker",
      chat: { description: "Deal chat with Hwan" },
    },
    {
      name: "another setup's legacy marker",
      error: "not this setup's legacy marker",
      chat: { description: creationMarker(request("describe-setup-2")) },
    },
    {
      name: "a new-form description with a wrong ref",
      error: "not this setup's legacy marker",
      chat: {
        description: `${CHAT_DESCRIPTION_TEXT} Ref: ${otherRef(setupRef(request()))}`,
      },
    },
    {
      name: "a chat the custody account did not create",
      error: "did not create",
      chat: { isCreator: false },
    },
    {
      name: "a forum",
      error: "did not create",
      chat: { isForum: true },
    },
    {
      name: "a setup without a chat",
      error: "no chat",
      setup: { chatId: undefined },
    },
    {
      name: "a receipt for a different request",
      error: "fingerprint conflicts",
      local: { fingerprint: "0".repeat(64), state: "ready", chatId: CHAT_ID },
    },
    {
      name: "a receipt naming a different chat",
      error: "chat conflicts",
      local: {
        fingerprint: requestFingerprint(request()),
        state: "ready",
        chatId: -1009999999999,
      },
    },
    {
      name: "the wrong custody account",
      error: "identity mismatch",
      self: 11,
    },
  ]
  for (const refusal of refusals)
    for (const run of [false, true])
      test(`refuses ${refusal.name}${run ? " with --run" : ""}`, async () => {
        const store = chatStore(refusal.chat)
        if (refusal.self !== undefined) {
          const self = refusal.self
          store.telegram.self = async () => self
        }
        await expect(
          rewriteChatDescription({
            setup: readySetup(refusal.setup),
            local: refusal.local ?? null,
            telegram: store.telegram,
            run,
          }),
        ).rejects.toThrow(refusal.error)
        expect(store.writes).toEqual([])
      })

  test("refuses when the description does not read back as written", async () => {
    const store = chatStore()
    store.telegram.setDescription = async (chatId, description) => {
      store.writes.push({ chatId, description })
    }
    await expect(
      rewriteChatDescription({
        setup: readySetup(),
        local: null,
        telegram: store.telegram,
        run: true,
      }),
    ).rejects.toThrow("did not read back")
  })
})
