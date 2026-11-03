import { expect, test } from "bun:test"
import { TOOL_HANDLERS } from "../lib/handlers.js"
import { TOOLS } from "../lib/tools.js"

const byName = Object.fromEntries(TOOLS.map((t) => [t.name, t]))
function props(name) {
  const t = byName[name] || {}
  const s = t.inputSchema || t.parameters || t.input_schema || t
  return s.properties || {}
}

test("include_rich_message_fields is exposed on the four read tools", () => {
  for (const name of [
    "slack_conversations_history",
    "slack_get_full_conversation",
    "slack_search_messages",
    "slack_get_thread",
  ]) {
    expect(byName[name], `tool ${name} exists`).toBeTruthy()
    expect(
      props(name).include_rich_message_fields?.type,
      `${name} exposes include_rich_message_fields`,
    ).toBe("boolean")
  }
})

test("every advertised tool has a dispatch handler and vice versa (drift guard, #30/#123)", () => {
  const advertised = TOOLS.map((t) => t.name).sort()
  const dispatchable = Object.keys(TOOL_HANDLERS).sort()
  expect(
    dispatchable,
    "TOOLS (lib/tools.js) and TOOL_HANDLERS (lib/handlers.js) must list the same tool names",
  ).toStrictEqual(advertised)
  for (const name of advertised) {
    expect(typeof TOOL_HANDLERS[name], `${name} handler is a function`).toBe(
      "function",
    )
  }
})

test("include_all_metadata is on history/replies tools but NOT on search (orthogonality guard)", () => {
  for (const name of [
    "slack_conversations_history",
    "slack_get_full_conversation",
    "slack_get_thread",
  ]) {
    expect(
      props(name).include_all_metadata?.type,
      `${name} exposes include_all_metadata`,
    ).toBe("boolean")
  }
  expect(
    props("slack_search_messages").include_all_metadata,
    "search.messages must NOT expose include_all_metadata (Slack search has no such option)",
  ).toBe(undefined)
})
