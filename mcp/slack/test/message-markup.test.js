/**
 * Slack markup in sends and edits (jiyuu issue 138).
 *
 * fixtures/message-markup-live.json holds what live Slack stored for one
 * text under each chat.postMessage and chat.update variant, captured with
 * scripts/probe-message-markup.js. The first tests pin that behavior; the
 * rest replay it through the stub so the edit handlers must send the args
 * that keep markup intact.
 */
import { afterEach, expect, test } from "bun:test"
import {
  handleUpdateMessage,
  handleUpdateMessageUnsafe,
} from "../lib/handlers.js"
import { clearSelfUserIdCache } from "../lib/message-ownership.js"
import {
  sanitizeMessage,
  scrubIds,
} from "../scripts/probe-message-markup.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"
import live from "./fixtures/message-markup-live.json"

let stub
afterEach(() => {
  stub?.restore()
  stub = undefined
  clearSelfUserIdCache()
})

const LINK = "<https://example.com|label #1>"
const ESCAPED_LINK = "&lt;https://example.com|label #1&gt;"
const USER = "<@U0SELF0001>"
const CHANNEL_MENTION = "<#C0CHANNEL01>"
const CODE = "`discord.gg/saidotfun`"
const SWALLOWED = "`<http://discord.gg/saidotfun`|discord.gg/saidotfun`>"

function variant(name) {
  const found = live.variants.find((v) => v.variant === name)
  if (!found) throw new Error(`fixture has no variant ${name}`)
  return found
}

function storedText(name) {
  return variant(name).stored.text
}

function richTextElements(name) {
  return variant(name).stored.blocks.flatMap((b) =>
    b.elements.flatMap((section) => section.elements),
  )
}

test("live: chat.update with no parse escapes <url|label> into literal text", () => {
  const text = storedText("update_default")
  expect(text).toContain(ESCAPED_LINK)
  expect(text).not.toContain(LINK)
  // Mentions survive the default; only the labeled link breaks.
  expect(text).toContain(USER)
  expect(text).toContain(CHANNEL_MENTION)
  expect(richTextElements("update_default")).not.toContainEqual(
    expect.objectContaining({ type: "link", url: "https://example.com" }),
  )
})

test("live: chat.update's default matches an explicit parse=client", () => {
  expect(storedText("update_parse_client")).toBe(storedText("update_default"))
  expect(storedText("post_parse_client")).toBe(storedText("update_default"))
})

test("live: chat.update with parse=none keeps links, mentions, and inline code", () => {
  const text = storedText("update_parse_none")
  expect(text).toContain(LINK)
  expect(text).toContain(USER)
  expect(text).toContain(CHANNEL_MENTION)
  expect(text).toContain(CODE)
  expect(text).toContain("a &amp; b &lt; c &gt; d")
  expect(text).toBe(storedText("post_parse_none"))
  expect(richTextElements("update_parse_none")).toContainEqual(
    expect.objectContaining({
      type: "link",
      url: "https://example.com",
      text: "label #1",
    }),
  )
})

test("live: parse=none leaves bare URLs and domains as plain text", () => {
  expect(storedText("update_parse_none")).toContain(
    "bare https://example.com/path and example.org",
  )
  expect(storedText("post_default")).toContain(
    "bare <https://example.com/path> and <http://example.org|example.org>",
  )
})

test("live: a default send auto-links a bare domain inside inline code", () => {
  const text = storedText("post_default")
  expect(text).toContain(LINK)
  expect(text).toContain(
    "`<http://discord.gg/saidotfun|discord.gg/saidotfun>`",
  )
  // Before punctuation, the link swallows the closing backtick.
  expect(text).toContain(SWALLOWED)
})

const SELF = "U0SELF0001"
const CHANNEL = "D0SELFDM01"
const NOW_SEC = Math.floor(Date.now() / 1000)
const OLD_TEXT = "draft before the edit"

/** Answer chat.update the way live Slack did for the parse value sent. */
function replayChatUpdate(params) {
  const name = {
    none: "update_parse_none",
    full: "update_parse_full",
    client: "update_parse_client",
  }[params.parse] ?? "update_default"
  if (params.text !== live.sent_text) {
    throw new Error("replay only covers the fixture's sent_text")
  }
  return {
    ok: true,
    channel: params.channel,
    ts: params.ts,
    text: variant(name).response_text,
  }
}

function replayStub(ts) {
  return stubSlackApi({
    "auth.test": { ok: true, user_id: SELF },
    "conversations.history": {
      ok: true,
      messages: [{ ts, user: SELF, text: OLD_TEXT }],
    },
    "chat.update": replayChatUpdate,
  })
}

for (const [name, handler, hoursAgo] of [
  ["slack_update_message", handleUpdateMessage, 0.25],
  ["slack_update_message_unsafe", handleUpdateMessageUnsafe, 24 * 8],
]) {
  test(`${name} sends parse=none and Slack keeps the markup`, async () => {
    const ts = `${NOW_SEC - Math.round(hoursAgo * 3600)}.000100`
    stub = replayStub(ts)

    const result = await handler({
      channel_id: CHANNEL,
      timestamp: ts,
      expected_text: OLD_TEXT,
      text: live.sent_text,
    })

    expect(result.isError).toBeUndefined()
    const payload = JSON.parse(result.content[0].text)
    expect(payload.status).toBe("updated")
    expect(payload.text).toBe(variant("update_parse_none").response_text)
    expect(payload.text).toContain(LINK)
    expect(payload.text).not.toContain(ESCAPED_LINK)
    const update = stub.calls.find((c) => c.method === "chat.update")
    expect(update.params.parse).toBe("none")
  })
}

test("the probe swaps real IDs for placeholders in text and blocks", () => {
  const replacements = { U0REAL0001: "U0SELF0001", C0REAL0001: "C0CHANNEL01" }
  const message = {
    ts: "1794974094.305739",
    user: "U0REAL0001",
    team: "T0REAL0001",
    client_msg_id: "abc",
    text: "<@U0REAL0001> in <#C0REAL0001>",
    blocks: [{
      type: "rich_text",
      elements: [{
        type: "rich_text_section",
        elements: [
          { type: "user", user_id: "U0REAL0001" },
          { type: "channel", channel_id: "C0REAL0001" },
        ],
      }],
    }],
  }

  const clean = sanitizeMessage(message, "TS_X", replacements)

  expect(Object.keys(clean).sort()).toStrictEqual(
    ["blocks", "edited", "text", "ts"],
  )
  expect(clean.ts).toBe("TS_X")
  expect(clean.text).toBe("<@U0SELF0001> in <#C0CHANNEL01>")
  expect(JSON.stringify(clean)).not.toContain("REAL")
  expect(scrubIds({ a: ["U0REAL0001", 3, null] }, replacements))
    .toStrictEqual({ a: ["U0SELF0001", 3, null] })
})

test("the saved fixture holds only placeholder IDs", () => {
  const json = JSON.stringify(live)
  expect(json).toContain("U0SELF0001")
  expect(json).toContain("C0CHANNEL01")
  expect(json.match(/\b[UC]0[A-Z0-9]{8,}\b/g).every((id) =>
    id === "U0SELF0001" || id === "C0CHANNEL01")).toBe(true)
})
