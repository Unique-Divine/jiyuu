import { expect, test } from "bun:test"
import { object } from "./decode.ts"

const root = new URL("../../", import.meta.url)

async function pathMappings(): Promise<Map<string, string[]>> {
  const raw: unknown = Bun.YAML.parse(
    await Bun.file(new URL(".github/labeler.yml", root)).text(),
  )
  const config = object(raw, "labeler config")
  const result = new Map<string, string[]>()
  for (const [name, value] of Object.entries(config)) {
    if (!Array.isArray(value) || value.length !== 1)
      throw new Error("Expected one mapping")
    const rules: readonly unknown[] = value
    const files = object(rules[0], "mapping")["changed-files"]
    if (!Array.isArray(files) || files.length !== 1)
      throw new Error("Expected one changed-files group")
    const groups: readonly unknown[] = files
    const patterns = object(groups[0], "group")["any-glob-to-any-file"]
    if (!Array.isArray(patterns)) throw new Error("Expected glob array")
    const globs: unknown[] = patterns
    result.set(
      name,
      globs.map((glob) => {
        if (typeof glob !== "string") throw new Error("Expected string glob")
        return glob
      }),
    )
  }
  return result
}

test("Telegram receives AI and both MCP zones; CLI paths stay outside that zone", async () => {
  const mappings = await pathMappings()
  const labels = (path: string) =>
    [...mappings]
      .filter(([, globs]) =>
        globs.some((glob) => new Bun.Glob(glob).match(path)),
      )
      .map(([name]) => name)
      .sort()

  expect(labels("mcp/telegram/telegram_mcp/tools/chats.py")).toEqual([
    "A-ai",
    "Z-mcp",
    "Z-mcp-telegram",
  ])
  expect(labels("mcp/discord/src/index.ts")).toEqual([
    "A-ai",
    "Z-mcp",
    "Z-mcp-discord",
  ])
  expect(labels("mcp/slack/src/server.js")).toEqual([
    "A-ai",
    "Z-mcp",
    "Z-mcp-slack",
  ])
  expect(labels("mcp/README.md")).toEqual(["A-ai", "Z-mcp"])
  expect(labels("ai-skills/gh-labels/SKILL.md")).toEqual([
    "A-ai",
    "Z-ai-skills",
  ])
  expect(labels("tmux-agent-watch/src/main.rs")).toEqual([
    "A-ai",
    "Z-tmux-agent-watch",
  ])
  expect(labels("ts-pkg/tg-sai/main.ts")).toEqual([])
  for (const path of [
    ".github/workflows/github-labels.yml",
    "scripts/github-labels/main.ts",
    "justfile",
    "mcp/slack/justfile",
    "local.just",
    "ci/test.just",
  ]) {
    expect(labels(path)).toContain("Z-ci")
  }
  expect(labels("gh-rev/src/main.rs")).toEqual([])
})

test("human label table and automation agree on all mapped names and paths", async () => {
  const mappings = await pathMappings()
  const doc = await Bun.file(new URL("GITHUB.md", root)).text()
  const documented = new Map<string, string[]>()
  for (const line of doc.split("\n")) {
    const cells = line.split("|").map((cell) => cell.trim())
    const name = /^`([AZ]-.+)`$/.exec(cells[1] ?? "")?.[1]
    if (!name) continue
    const paths = [...(cells[3] ?? "").matchAll(/`([^`]+)`/g)].map(
      (match) => match[1],
    )
    if (!paths.every((path) => typeof path === "string"))
      throw new Error("Missing table path")
    documented.set(name, paths)
  }
  expect(documented).toEqual(mappings)
  expect(
    [...mappings.keys()].every(
      (name) => name.startsWith("A-") || name.startsWith("Z-"),
    ),
  ).toBe(true)
  const skill = await Bun.file(
    new URL("ai-skills/gh-labels/SKILL.md", root),
  ).text()
  expect(skill).toContain(
    "`A-ai`: (#eab6cb) | Area: AI tooling, MCP servers, agent skills, and agent workflow tools",
  )
  expect(doc).toContain(
    "Area: AI tooling, MCP servers, agent skills, and agent workflow tools",
  )
})

test("privileged label workflow orders path labels before inheritance and checks out only trusted code", async () => {
  const workflow = object(
    Bun.YAML.parse(
      await Bun.file(
        new URL(".github/workflows/github-labels.yml", root),
      ).text(),
    ),
    "workflow",
  )
  const jobs = object(workflow.jobs, "jobs")
  const pathJob = object(jobs["label-paths"], "path job")
  const inheritJob = object(jobs["inherit-issue-labels"], "inherit job")
  expect(inheritJob.needs).toBe("label-paths")
  expect(
    object(workflow.concurrency, "concurrency")["cancel-in-progress"],
  ).toBe(false)
  if (!Array.isArray(pathJob.steps) || !Array.isArray(inheritJob.steps))
    throw new Error("Missing workflow steps")
  const pathSteps: readonly unknown[] = pathJob.steps
  const inheritSteps: readonly unknown[] = inheritJob.steps
  expect(
    pathSteps.some((step) =>
      String(object(step, "step").uses).includes("checkout"),
    ),
  ).toBe(false)
  expect(
    object(object(pathSteps[0], "step").with, "inputs")["sync-labels"],
  ).toBe(true)
  const checkout = object(inheritSteps[0], "checkout")
  expect(object(checkout.with, "checkout inputs").ref).toBe(
    "${{ github.event.repository.default_branch }}",
  )
  expect(object(inheritJob.permissions, "inherit permissions")["issues"]).toBe(
    "write",
  )
})
