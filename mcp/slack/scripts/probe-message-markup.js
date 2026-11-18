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
 * Never prints tokens or cookies: the client loads them itself, and the
 * output keeps only message fields.
 */

import { writeFileSync } from "node:fs"
import { slackAPI } from "../lib/slack-client.js"

export const CHANNEL_PLACEHOLDER = "D_SELF_DM"

/** Markup cases from issue 138: link, user and channel mentions, inline
 * code with a bare domain (alone and before punctuation), bare URLs and
 * domains outside code, and raw &, <, > characters. */
export function probeText(selfId) {
  return [
    "jiyuu#138 probe (deleting):",
    "<https://example.com|label #1>",
    `<@${selfId}>`,
    "<#C03DD94GR8W>",
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

/** Keep only the stored fields the fixture documents; drop ids and tokens. */
export function sanitizeMessage(message, tsPlaceholder) {
  return {
    ts: tsPlaceholder,
    text: message?.text ?? null,
    blocks: message?.blocks ?? null,
    edited: message?.edited ? true : undefined,
  }
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

async function runVariant(variant, channel, text, posted) {
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
    stored: sanitizeMessage(stored, `TS_${variant.name.toUpperCase()}`),
  }
}

async function main() {
  const argv = process.argv.slice(2)
  const run = argv.includes("--run")
  const outIdx = argv.indexOf("--out")
  const out = outIdx >= 0 ? argv[outIdx + 1] : null

  if (!run) {
    console.log("Dry run. Pass --run to post to your own self-DM.")
    console.log(`Text: ${probeText("<self>")}`)
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

  const text = probeText(selfId)
  const posted = []
  const results = []
  try {
    for (const v of VARIANTS) {
      results.push(await runVariant(v, channel, text, posted))
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

  const fixture = {
    source: "live Slack Web API, scripts/probe-message-markup.js",
    captured_at: new Date().toISOString().slice(0, 10),
    channel: CHANNEL_PLACEHOLDER,
    sent_text: text,
    variants: results,
  }
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
