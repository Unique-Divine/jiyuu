import { expect, test } from "bun:test"
import { existsSync, readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { JiyuuMcpInfo, formatNotice } from "../lib/jiyuuMcpInfo.ts"
import { callTool } from "../src/server.js"

const packageRoot = fileURLToPath(new URL("..", import.meta.url))
const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"))
const behind = {
  name: "@uniquedivine/mcp-slack",
  runningVersion: "4.4.3",
  latestVersion: "4.5.0",
  checkedAt: "2026-11-17T12:00:00Z",
  source: "live",
}

// Never reach the registry or the user's cache from tests.
const offlineInfo = (status = null) =>
  new JiyuuMcpInfo("slack", packageRoot, ["SLACK_"], {
    startup: async () => status,
    live: async () => status,
  })

const mcpInfoRequest = { params: { name: "mcp_info", arguments: {} } }

test("mcp_info reports the package version, provenance and env names only", async () => {
  const secret = "xoxc-fake-slack-token-value"
  const info = offlineInfo()
  const result = await info.callTool({ SLACK_TOKEN: secret })
  const report = JSON.parse(result.content[0].text)

  expect(report.server).toBe("slack")
  expect(report.name).toBe(pkg.name)
  expect(report.version).toBe(pkg.version)
  expect(report.provenance.kind).toBe("source-checkout")
  expect(report.envVarsSet).toEqual(["SLACK_TOKEN"])
  expect(result.content[0].text).not.toContain(secret)
})

test("the update notice rides on the first successful result only", async () => {
  const info = offlineInfo(behind)
  info.notifier.start()
  await info.notifier.status(1000)

  const unknown = await callTool({ params: { name: "no_such_tool", arguments: {} } }, info)
  const first = await callTool(mcpInfoRequest, info)
  const second = await callTool(mcpInfoRequest, info)

  expect(unknown.isError).toBe(true)
  expect(unknown.content).toHaveLength(1)
  expect(first.content).toHaveLength(2)
  expect(first.content[1]).toEqual({ type: "text", text: formatNotice(behind) })
  expect(JSON.parse(first.content[0].text).update.updateAvailable).toBe(true)
  expect(second.content).toHaveLength(1)
})

test("the Slack and Discord copies of jiyuuMcpInfo.ts are identical", () => {
  const slack = fileURLToPath(new URL("../lib/jiyuuMcpInfo.ts", import.meta.url))
  const discord = fileURLToPath(new URL("../../discord/src/jiyuuMcpInfo.ts", import.meta.url))
  if (!existsSync(discord)) return // Outside the Jiyuu monorepo.
  expect(readFileSync(slack, "utf8")).toBe(readFileSync(discord, "utf8"))
})

test("mcp_info runs the live check on every call, not the startup result", async () => {
  let calls = 0
  const current = { ...behind, runningVersion: "4.5.0", source: "live" }
  const info = new JiyuuMcpInfo("slack", packageRoot, ["SLACK_"], {
    startup: async () => behind,
    live: async () => {
      calls++
      return current
    },
  })

  const first = JSON.parse((await callTool(mcpInfoRequest, info)).content[0].text)
  await callTool(mcpInfoRequest, info)

  expect(calls).toBe(2)
  expect(first.update).toMatchObject({
    latestVersion: "4.5.0",
    updateAvailable: false,
    pinCommand: null,
    source: "live",
  })
})
