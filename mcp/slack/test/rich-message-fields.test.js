import { expect, test } from "bun:test"
import {
  RICH_MESSAGE_KEYS,
  withRichMessageFields,
} from "../lib/rich-message-fields.js"

test("disabled: base output is returned unchanged (no rich keys added)", () => {
  const base = { ts: "1", text: "hi" }
  const msg = {
    attachments: [{ text: "x" }],
    blocks: [{}],
    subtype: "bot_message",
  }
  const out = withRichMessageFields(base, msg, false)
  expect(out).toBe(base) // same ref; mutate-in-place contract
  expect(out).toStrictEqual({ ts: "1", text: "hi" })
})

test("enabled: only the rich keys present on the message are merged", () => {
  const base = { ts: "1", text: "" }
  const msg = {
    attachments: [{ text: "alert body" }],
    bot_id: "B1",
    app_id: "A1",
  }
  const out = withRichMessageFields(base, msg, true)
  expect(out.attachments).toStrictEqual([{ text: "alert body" }])
  expect(out.bot_id).toBe("B1")
  expect(out.app_id).toBe("A1")
  expect(out.ts).toBe("1") // base fields preserved
  for (const k of [
    "blocks",
    "metadata",
    "files",
    "reactions",
    "subtype",
    "team",
  ]) {
    expect(!(k in out), `${k} should be skipped when absent`).toBeTruthy()
  }
})

test("enabled but msg falsy: base returned unchanged", () => {
  const base = { ts: "1" }
  expect(withRichMessageFields(base, null, true)).toBe(base)
  expect(base).toStrictEqual({ ts: "1" })
})

test("present-but-falsy value is copied; only undefined is skipped", () => {
  const out = withRichMessageFields(
    {},
    { attachments: [], reactions: undefined },
    true,
  )
  expect("attachments" in out).toBeTruthy()
  expect(out.attachments).toStrictEqual([])
  expect(!("reactions" in out), "undefined value must be skipped").toBeTruthy()
})

test("RICH_MESSAGE_KEYS covers documented fields + bot/app markers", () => {
  for (const k of [
    "attachments",
    "blocks",
    "metadata",
    "files",
    "reactions",
    "subtype",
    "bot_id",
    "app_id",
    "team",
  ]) {
    expect(RICH_MESSAGE_KEYS.includes(k), `missing ${k}`).toBeTruthy()
  }
})
