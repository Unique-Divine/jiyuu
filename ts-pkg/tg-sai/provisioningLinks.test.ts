import { describe, expect, test } from "bun:test"
import { readProvisioningLinks } from "./provisioningRuntime"

const chatId = -1004387376666
const primary = {
  link: "https://t.me/+fixturePrimary",
  isPrimary: true,
  isMyLink: true,
  isRevoked: false,
  endDate: null as Date | null,
}
const reader = () => ({
  history: async () => [{ id: 47, chat: { id: chatId } }],
  primaryInvite: async () => ({ ...primary }),
})

describe("provisioning completion links", () => {
  test("uses an observed message from this group and reads its reusable primary invite", async () => {
    expect(await readProvisioningLinks(chatId, reader())).toEqual({
      openGroupUrl: "https://t.me/c/4387376666/47",
      primaryInviteLink: primary.link,
    })
  })

  test("does not fabricate a message anchor for empty or unrelated history", async () => {
    for (const messages of [
      [],
      [{ id: 47, chat: { id: -1000000000456 } }],
      [{ id: 0, chat: { id: chatId } }],
      [{ id: 2_147_483_648, chat: { id: chatId } }],
    ]) {
      const reads = { ...reader(), history: async () => messages }
      expect(await readProvisioningLinks(chatId, reads)).toEqual({
        primaryInviteLink: primary.link,
      })
    }
  })

  test("omits revoked, censored, non-primary, expired, and non-Telegram invitations", async () => {
    for (const patch of [
      { isRevoked: true },
      { isPrimary: false },
      { isMyLink: false },
      { endDate: new Date(0) },
      { link: "https://t.me/+fixture..." },
      { link: "http://t.me/+fixture" },
      { link: "https://t.me.evil/+fixture" },
      { link: "https://t.me/+fixture?next=evil" },
      { link: "https://t.me/+tiny" },
      { link: `https://t.me/+${"a".repeat(129)}` },
    ]) {
      const reads = {
        ...reader(),
        primaryInvite: async () => ({ ...primary, ...patch }),
      }
      expect(await readProvisioningLinks(chatId, reads)).toEqual({
        openGroupUrl: "https://t.me/c/4387376666/47",
      })
    }
  })

  test("preserves either available link when the other lookup fails", async () => {
    const fail = async (): Promise<never> => {
      throw new Error("unavailable")
    }
    expect(await readProvisioningLinks(chatId, { ...reader(), history: fail })).toEqual({
      primaryInviteLink: primary.link,
    })
    expect(await readProvisioningLinks(chatId, { ...reader(), primaryInvite: fail })).toEqual({
      openGroupUrl: "https://t.me/c/4387376666/47",
    })
  })

  test("bounds both reads and returns available metadata without awaiting stuck RPCs", async () => {
    const signals: AbortSignal[] = []
    const stuck = (signal: AbortSignal): Promise<never> => {
      signals.push(signal)
      return new Promise(() => {})
    }
    expect(await readProvisioningLinks(chatId, { ...reader(), history: stuck }, 5)).toEqual({
      primaryInviteLink: primary.link,
    })
    expect(await readProvisioningLinks(chatId, { history: stuck, primaryInvite: stuck }, 5)).toEqual({})
    expect(signals).toHaveLength(3)
    expect(signals.every((signal) => signal.aborted)).toBe(true)
  })

  test("rejects non-supergroup chat IDs without attempting reads", async () => {
    const unexpected = async (): Promise<never> => {
      throw new Error("must not read")
    }
    expect(await readProvisioningLinks(-123, {
      history: unexpected,
      primaryInvite: unexpected,
    })).toEqual({})
  })
})
