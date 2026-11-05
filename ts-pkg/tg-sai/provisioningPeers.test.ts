import { describe, expect, test } from "bun:test"
import { resolveProvisioningPeer } from "./provisioningRuntime"

describe("custody participant resolution with a fresh session", () => {
  for (const [id, handle] of [
    [1912700300, "uniquedivine"],
    [5965882418, "lilnaptime"],
    [472528179, "erickpinos"],
  ] as const) {
    test(`resolves pinned ${handle} without a cached numeric access hash`, async () => {
      const calls: Array<string | number> = []
      const getUser = async (peer: string | number) => {
        calls.push(peer)
        if (typeof peer === "number") throw new Error("PEER_ID_INVALID")
        if (peer !== handle) throw new Error("Unexpected alias")
        return { id, isBot: false }
      }
      for (const peer of [id, String(id), `@${handle}`, handle.toUpperCase()]) {
        expect(await resolveProvisioningPeer(getUser, peer)).toEqual({
          id,
          isBot: false,
        })
      }
      expect(calls).toEqual(Array(4).fill(handle))
    })
  }

  test("a reassigned default handle cannot change the intended participant", async () => {
    for (const peer of [5965882418, "5965882418", "@lilnaptime"]) {
      await expect(
        resolveProvisioningPeer(
          async () => ({ id: 12345, isBot: false }),
          peer,
        ),
      ).rejects.toThrow("participant identity mismatch")
    }
  })

  test("default participants must remain ordinary user accounts", async () => {
    await expect(
      resolveProvisioningPeer(
        async () => ({ id: 472528179, isBot: true }),
        472528179,
      ),
    ).rejects.toThrow("must be a user account")
  })

  test("unknown numeric peers retain their exact numeric identity", async () => {
    const calls: Array<string | number> = []
    const getUser = async (peer: string | number) => {
      calls.push(peer)
      return { id: 987654321, isBot: false }
    }
    await resolveProvisioningPeer(getUser, "987654321")
    expect(calls).toEqual([987654321])
    await expect(
      resolveProvisioningPeer(
        async () => ({ id: 123, isBot: false }),
        987654321,
      ),
    ).rejects.toThrow("participant identity mismatch")
  })

  test("other handles and Telegram errors keep their existing behavior", async () => {
    const calls: Array<string | number> = []
    expect(
      await resolveProvisioningPeer(async (peer) => {
        calls.push(peer)
        return { id: 456, isBot: true }
      }, "@another_bot"),
    ).toEqual({ id: 456, isBot: true })
    expect(calls).toEqual(["another_bot"])
    const flood = Object.assign(new Error("FLOOD_WAIT_30"), { seconds: 30 })
    await expect(
      resolveProvisioningPeer(async () => {
        throw flood
      }, 5965882418),
    ).rejects.toBe(flood)
  })
})
