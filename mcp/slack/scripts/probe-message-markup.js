#!/usr/bin/env bun
/**
 * Live probe: how chat.postMessage and chat.update store Slack markup.
 *
 * Posts throwaway messages to the signed-in user's own DM (self-DM only),
 * applies each variant below, reads every message back with
 * conversations.history, deletes it, and writes the sanitized stored
 * `text` and `blocks` per variant as JSON. Used to capture
 * test/fixtures/message-markup-live.json (jiyuu issue 138).
 *
 * Dry run by default: prints the plan and touches nothing.
 *
 *   bun scripts/probe-message-markup.js            # dry run
 *   bun scripts/probe-message-markup.js --run      # post, read, delete
 *   bun scripts/probe-message-markup.js --run --out test/fixtures/x.json
 *
 * The probe mentions the signed-in user (from auth.test) and one public
 * channel they belong to (from users.conversations). The saved JSON
 * replaces both IDs with placeholders and keeps only message text and
 * blocks. Never prints tokens or cookies: the client loads them itself.
 */

import { writeFileSync } from "node:fs"
import { slackAPI } from "../lib/slack-client.js"

export const CHANNEL_PLACEHOLDER = "D_SELF_DM"
export const USER_PLACEHOLDER = "U0SELF0001"
export const MENTION_CHANNEL_PLACEHOLDER = "C0CHANNEL01"

/** Markup cases from issue 138: link, user and channel mentions, inline
 * code with a bare domain (alone and before punctuation), bare URLs and
 * domains outside code, and raw &, <, > characters. */
export function probeText(selfId, mentionChannelId) {
  return [
    "jiyuu#138 probe (deleting):",
    "<https://example.com|label #1>",
    `<@${selfId}>`,
    `<#${mentionChannelId}>`,
    "`discord.gg/saidotfun`",
    "(`discord.gg/saidotfun`).",
    "bare https://example.com/path and example.org",
    "a & b < c > d",
  ].join(" ")
}

/**
 * Each variant posts a fresh message, then optionally updates it.
 * `post` and `update` are extra args merged over channel/ts/text.
 */
export const VARIANTS = [
  { name: "post_default", post: {} },
  { name: "post_parse_none", post: { parse: "none" } },
  { name: "post_parse_client", post: { parse: "client" } },
  { name: "update_default", post: {}, update: {} },
  { name: "update_parse_none", post: {}, update: { parse: "none" } },
  { name: "update_parse_client", post: {}, update: { parse: "client" } },
  { name: "update_parse_full", post: {}, update: { parse: "full" } },
]

/**
 * Replace every occurrence of each real ID with its placeholder, in all
 * strings inside `value`.
 * @param {unknown} value
 * @param {Record<string, string>} replacements real ID to placeholder
 */
export function scrubIds(value, replacements) {
  if (typeof value === "string") {
    let out = value
    for (const [real, placeholder] of Object.entries(replacements)) {
      out = out.split(real).join(placeholder)
    }
    return out
  }
  if (Array.isArray(value)) return value.map((v) => scrubIds(v, replacements))
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([k, v]) => [k, scrubIds(v, replacements)]),
    )
  }
  return value
}

/**
 * Keep only the stored text, blocks, and edited flag; drop the message's
 * user, team, client_msg_id, and other fields. Real user and channel IDs
 * inside text and blocks become placeholders via `replacements`.
 */
export function sanitizeMessage(message, tsPlaceholder, replacements = {}) {
  return scrubIds({
    ts: tsPlaceholder,
    text: message?.text ?? null,
    blocks: message?.blocks ?? null,
    edited: message?.edited ? true : undefined,
  }, replacements)
}

async function readBack(channel, ts) {
  const res = await slackAPI("conversations.history", {
    channel,
    latest: ts,
    oldest: ts,
    inclusive: true,
    limit: 1,
  })
  return res.messages?.find((m) => m.ts === ts) ?? null
}

async function runVariant(variant, channel, text, posted, replacements) {
  const post = await slackAPI("chat.postMessage", {
    channel,
    text,
    ...variant.post,
  })
  posted.push({ variant: variant.name, channel: post.channel, ts: post.ts })
  let apiText = post.message?.text ?? null
  if (variant.update) {
    const upd = await slackAPI("chat.update", {
      channel: post.channel,
      ts: post.ts,
      text,
      ...variant.update,
    })
    apiText = upd.text ?? upd.message?.text ?? null
  }
  const stored = await readBack(post.channel, post.ts)
  return {
    variant: variant.name,
    post_args: variant.post,
    update_args: variant.update ?? null,
    response_text: apiText,
    stored: sanitizeMessage(
      stored,
      `TS_${variant.name.toUpperCase()}`,
      replacements,
    ),
  }
}

async function main() {
  const argv = process.argv.slice(2)
  const run = argv.includes("--run")
  const outIdx = argv.indexOf("--out")
  const out = outIdx >= 0 ? argv[outIdx + 1] : null

  if (!run) {
    console.log("Dry run. Pass --run to post to your own self-DM.")
    console.log(`Text: ${probeText("<self>", "<public channel>")}`)
    for (const v of VARIANTS) {
      console.log(`- ${v.name}: post ${JSON.stringify(v.post)}` +
        (v.update ? `, update ${JSON.stringify(v.update)}` : ""))
    }
    return
  }

  const auth = await slackAPI("auth.test", {})
  const selfId = auth.user_id
  // Self-DM only: open a DM with the signed-in user and nobody else.
  const open = await slackAPI("conversations.open", { users: selfId })
  const channel = open.channel?.id
  if (!channel?.startsWith("D")) {
    throw new Error(`conversations.open did not return a DM: ${channel}`)
  }
  console.log(`Self-DM ${channel} for ${selfId}`)

  // Any public channel the user is in works for the <#C…> mention.
  const member = await slackAPI("users.conversations", {
    types: "public_channel",
    exclude_archived: true,
    limit: 1,
  })
  const mentionChannelId = member.channels?.[0]?.id
  if (!mentionChannelId) {
    throw new Error("users.conversations returned no public channel")
  }

  const replacements = {
    [selfId]: USER_PLACEHOLDER,
    [mentionChannelId]: MENTION_CHANNEL_PLACEHOLDER,
    [channel]: CHANNEL_PLACEHOLDER,
  }
  const text = probeText(selfId, mentionChannelId)
  const posted = []
  const results = []
  try {
    for (const v of VARIANTS) {
      results.push(await runVariant(v, channel, text, posted, replacements))
    }
  } finally {
    for (const p of posted) {
      try {
        await slackAPI("chat.delete", { channel: p.channel, ts: p.ts })
        p.deleted = true
      } catch (error) {
        p.deleted = false
        p.error = error.message
      }
      console.log(`${p.deleted ? "deleted" : "NOT DELETED"} ` +
        `${p.channel} ${p.ts} (${p.variant})`)
    }
  }

  const fixture = scrubIds({
    source: "live Slack Web API, scripts/probe-message-markup.js",
    captured_at: new Date().toISOString().slice(0, 10),
    channel: CHANNEL_PLACEHOLDER,
    sent_text: text,
    variants: results,
  }, replacements)
  const json = JSON.stringify(fixture, null, 2) + "\n"
  if (out) writeFileSync(out, json)
  else console.log(json)
}

if (import.meta.main) {
  main().catch((error) => {
    console.error(`probe failed: ${error.message}`)
    process.exit(1)
  })
}
