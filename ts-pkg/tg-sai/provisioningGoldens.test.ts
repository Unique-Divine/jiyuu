import { describe, expect, test } from "bun:test"
import { readdir, readFile } from "node:fs/promises"
import { join } from "node:path"
import { normalizeProvisioningMember } from "./provisioningRuntime"
import { memberShape, requireExactCaptureDialog, sanitizedProvisioningGolden, type MemberShape } from "./provisioningGoldens"
import type { Member } from "./provisioning"
import { BOT_BASELINE_RIGHTS, TEAM_ADMIN_RIGHTS } from "./provisioning"
import { chatDescription, creationMarker } from "./provisioning"
import type { ProvisionRequest } from "./provisioning"

// Synthetic shapes exercise the installed mtcute types. Live evidence is added
// separately after an authorized throwaway group exists.
const shapes: { subject: string; value: MemberShape | null; role: Member["role"] }[] = [
  { subject: "custody", value: { status: "creator", isMember: true,
    permissions: { _: "chatAdminRights", changeInfo: true, addAdmins: true } }, role: "creator" },
  { subject: "bot", value: { status: "admin", isMember: true,
    permissions: { _: "chatAdminRights", inviteUsers: true, addAdmins: false } }, role: "admin" },
  { subject: "ordinary", value: { status: "member", isMember: true, permissions: null }, role: "member" },
  { subject: "privacy_blocked", value: null, role: "absent" },
  { subject: "restricted_member", value: { status: "restricted", isMember: true, permissions: null }, role: "member" },
  { subject: "banned", value: { status: "banned", isMember: false, permissions: null }, role: "absent" },
]

describe("Provisioning capture adapter shapes", () => {
  test("dialog cache discovery requires the exact ID and stops at the bound", async () => {
    let read = 0
    async function* dialogs() {
      for (const id of [-1001, -1002, -1003]) {
        read++
        yield { peer: { id }, title: "identical title" }
      }
    }
    await requireExactCaptureDialog(dialogs(), -1002)
    expect(read).toBe(2)
    await expect(requireExactCaptureDialog(dialogs(), -9999)).rejects.toThrow("Exact capture group")
    read = 0
    await expect(requireExactCaptureDialog(dialogs(), -1003, 2)).rejects.toThrow("Exact capture group")
    expect(read).toBe(2)
  })
  for (const row of shapes) test(row.subject, () => {
    const sanitized = memberShape(row.value)
    expect(normalizeProvisioningMember(sanitized).role).toBe(row.role)
    expect(normalizeProvisioningMember(sanitized).rights).not.toHaveProperty("_")
    if (row.subject === "bot") expect(normalizeProvisioningMember(sanitized).rights.addAdmins).toBe(false)
  })
  test("capture omits usable links, raw peers, IDs, names, and marker text", () => {
    const fixture = sanitizedProvisioningGolden({
      capturedAt: "2026-11-05T00:00:00.000Z",
      chat: { chatType: "supergroup", isForum: false, isCreator: true, bio: "sai-setup:secret-marker" },
      expectedMarker: "sai-setup:secret-marker", members: shapes,
      invites: [{ isMyLink: true, isRevoked: false, approvalNeeded: true,
        isPrimary: false, endDate: new Date("2026-11-06T00:00:00Z"),
        usageLimit: Infinity, usage: 0, pendingApprovals: 0,
        // Deliberately supply extra private fields to prove they are dropped.
        ...{ link: "https://t.me/+secret", raw: { authKey: "private" } },
      }],
    })
    const json = JSON.stringify(fixture)
    for (const privateText of ["t.me", "secret", "authKey", '"raw"', '"link"'])
      expect(json).not.toContain(privateText)
    expect(fixture.chat.markerMatches).toBe(true)
    expect(fixture.invitations.entries[0]?.usageLimit).toBeNull()
    expect(fixture.invitations.entries[0]?.approvalNeeded).toBe(true)
  })
  test("capture matches the legacy marker and the new description by ref", () => {
    const request: ProvisionRequest = { requestId: "golden-setup", custodyProfile: "custody",
      expectedUserId: 10, title: "Sai golden", participants: [21], admins: [],
      botRightsProfile: "baseline", dealIds: [] }
    const capture = (bio: string) => sanitizedProvisioningGolden({
      capturedAt: "2026-11-10T00:00:00.000Z",
      chat: { chatType: "supergroup", isForum: false, isCreator: true, bio },
      expectedMarker: creationMarker(request), members: [], invites: [],
    })
    expect(capture(creationMarker(request)).chat.markerMatches).toBe(true)
    const fresh = capture(chatDescription(request))
    expect(fresh.chat.markerMatches).toBe(true)
    expect(JSON.stringify(fresh)).not.toContain("Ref:")
    const other = { ...request, requestId: "golden-other" }
    expect(capture(chatDescription(other)).chat.markerMatches).toBe(false)
  })
  test("saved live fixtures preserve owner, bot admin, and member mapping", async () => {
    const directory = join(import.meta.dir, "fixtures", "provisioning")
    const names = await readdir(directory).catch((error: NodeJS.ErrnoException) => {
      if (error.code === "ENOENT") return []
      throw error
    })
    const fixtureNames = names.filter(name => name.endsWith(".json"))
    expect(fixtureNames).toContain("custody-live.json")
    for (const name of fixtureNames) {
      const raw = await readFile(join(directory, name), "utf8")
      const fixture = JSON.parse(raw) as ReturnType<typeof sanitizedProvisioningGolden>
      expect(fixture.format).toBe("tg-sai-provisioning-golden/v1")
      expect(fixture.provenance.kind).toBe("live-readback")
      expect(fixture.chat).toEqual({ chatType: "supergroup", isForum: false, isCreator: true, markerMatches: true })
      expect(raw).not.toMatch(/https?:\/\/|sai-setup:|authKey|apiHash|sessionString/)
      const owner = fixture.members.find(member => member.subject === "custody")
      const bot = fixture.members.find(member => member.subject === "bot")
      expect(normalizeProvisioningMember(owner?.value ?? null).role).toBe("creator")
      expect(normalizeProvisioningMember(bot?.value ?? null).role).toBe("admin")
      const botRights = normalizeProvisioningMember(bot?.value ?? null).rights
      for (const forbidden of ["changeInfo", "banUsers", "addAdmins", "deleteMessages", "anonymous"])
        expect(Boolean(botRights[forbidden])).toBe(false)
      for (const [right, expected] of Object.entries(BOT_BASELINE_RIGHTS))
        expect(Boolean(botRights[right])).toBe(expected)
      for (const member of fixture.members.filter(member => member.subject.startsWith("required_admin_"))) {
        expect(normalizeProvisioningMember(member.value).role).toBe("admin")
        const rights = normalizeProvisioningMember(member.value).rights
        for (const [right, expected] of Object.entries(TEAM_ADMIN_RIGHTS))
          expect(Boolean(rights[right])).toBe(expected)
      }
    }
  })
})
