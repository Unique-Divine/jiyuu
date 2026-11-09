import { describe, expect, test } from "bun:test"
import {
  decodeClosingPage,
  decodeLabelPages,
  object,
  parseJson,
} from "./decode.ts"
import { executePlan, PartialWriteError } from "./execute.ts"
import fixture from "./fixtures/inheritance.json"
import { GhClient, type RunCommand } from "./github.ts"
import { parseOptions } from "./main.ts"
import { discoverPlan, dryRun, eligibleLabel } from "./plan.ts"
import type { Command } from "./types.ts"

const repo = "example/tools"
const sourcePath = `repos/${repo}/issues/12/labels?per_page=100`
const issuePath = (number: number) =>
  `repos/${repo}/issues/${number}/labels?per_page=100`

function responses(replacements: ReadonlyMap<string, unknown> = new Map()): {
  client: GhClient
  calls: Command[]
} {
  const calls: Command[] = []
  const byPath = new Map<string, unknown>([
    ["graphql", fixture.closing],
    [sourcePath, fixture.sourceLabels],
    [issuePath(4), fixture.issue4],
    [issuePath(9), fixture.issue9],
    ...replacements,
  ])
  return {
    calls,
    client: new GhClient(async (command) => {
      calls.push(command)
      const key = command.argv.find(
        (arg) => arg === "graphql" || arg.startsWith("repos/"),
      )
      if (!key || !byPath.has(key)) throw new Error("Unexpected GitHub read")
      if (command.argv.includes("POST"))
        throw new Error("Discovery must not write")
      return JSON.stringify(byPath.get(key))
    }),
  }
}

describe("GitHub response to exact dry-run plan", () => {
  test("golden fixture validates raw responses and emits sorted additive commands", async () => {
    const { client, calls } = responses()
    const result = dryRun(await discoverPlan(client, repo, 12))

    expect<unknown>(result).toEqual(fixture.expected)
    expect(calls).toHaveLength(4)
    expect(calls[0]?.argv).toEqual(["gh", "api", "graphql", "--input", "-"])
    expect(
      calls
        .slice(1)
        .every(
          (call) =>
            call.argv.includes("--paginate") && call.argv.includes("--slurp"),
        ),
    ).toBe(true)
    expect(
      calls.some((call) => JSON.stringify(call).includes("other/private")),
    ).toBe(false)
    const queryPayload = object(parseJson(calls[0]?.input ?? ""), "query")
    expect(queryPayload.variables).toEqual({
      owner: "example",
      name: "tools",
      number: 12,
      cursor: null,
    })
  })

  test("uppercase families are generic and status and priority never inherit", () => {
    for (const name of ["A-ai", "Z-mcp", "M-launch", "Q-future"])
      expect(eligibleLabel(name)).toBe(true)
    for (const name of [
      "S-impl",
      "prio-p2-normal",
      "bug",
      "a-ai",
      "Z-",
      "",
      "AA-ai",
    ]) {
      expect(eligibleLabel(name)).toBe(false)
    }
  })

  test("source and closing-issue pagination advance independently", async () => {
    const calls: Command[] = []
    const first = structuredClone(fixture.closing)
    first.data.repository.pullRequest.closingIssuesReferences.nodes =
      Array.from({ length: 100 }, () => ({
        number: 4,
        repository: { nameWithOwner: repo },
      }))
    first.data.repository.pullRequest.closingIssuesReferences.pageInfo = {
      hasNextPage: true,
      endCursor: "cursor-1",
    }
    const second = structuredClone(fixture.closing)
    second.data.repository.pullRequest.closingIssuesReferences.nodes = [
      { number: 9, repository: { nameWithOwner: repo } },
    ]
    const source = [
      Array.from({ length: 100 }, (_, i) => ({ name: `ordinary-${i}` })),
      [{ name: "Z-last-page" }],
    ]
    let graphqlCalls = 0
    const github = new GhClient(async (command) => {
      calls.push(command)
      if (command.argv.includes("graphql")) {
        const payload = object(parseJson(command.input ?? ""), "query")
        const vars = object(payload.variables, "variables")
        expect(vars.cursor).toBe(graphqlCalls === 0 ? null : "cursor-1")
        return JSON.stringify(graphqlCalls++ === 0 ? first : second)
      }
      if (command.argv.includes(sourcePath)) return JSON.stringify(source)
      if (command.argv.includes(issuePath(4))) return "[[]]"
      if (command.argv.includes(issuePath(9))) return "[[]]"
      throw new Error("Unexpected request")
    })

    const plan = await discoverPlan(github, repo, 12)

    expect(plan.sourceLabels).toEqual(["Z-last-page"])
    expect(plan.additions.map((a) => a.number)).toEqual([4, 9])
    expect(graphqlCalls).toBe(2)
    expect(calls).toHaveLength(5)
  })

  test("no targets and already-present labels produce no commands", async () => {
    const empty = structuredClone(fixture.closing)
    empty.data.repository.pullRequest.closingIssuesReferences.nodes = []
    const { client } = responses(new Map([["graphql", empty]]))
    expect(dryRun(await discoverPlan(client, repo, 12)).commands).toEqual([])
    const { client: populated } = responses(
      new Map([
        [issuePath(4), fixture.sourceLabels],
        [issuePath(9), fixture.sourceLabels],
      ]),
    )
    expect(dryRun(await discoverPlan(populated, repo, 12)).commands).toEqual([])
  })
})

describe("fail closed before writes", () => {
  test("rejects unmerged source and another source repository", async () => {
    const unmerged = structuredClone(fixture.closing)
    unmerged.data.repository.pullRequest.merged = false
    const { client, calls } = responses(new Map([["graphql", unmerged]]))
    await expect(discoverPlan(client, repo, 12)).rejects.toThrow("not merged")
    expect(calls).toHaveLength(1)
    await expect(
      discoverPlan(responses().client, "different/repo", 12),
    ).rejects.toThrow("different source")
  })

  test.each([
    {},
    { data: null },
    { errors: [{ message: "private detail" }], data: fixture.closing.data },
    { data: { repository: { nameWithOwner: repo, pullRequest: null } } },
  ])("rejects missing GraphQL data and errors", async (value) => {
    const { client, calls } = responses(new Map([["graphql", value]]))
    await expect(discoverPlan(client, repo, 12)).rejects.toThrow()
    expect(calls).toHaveLength(1)
  })

  test.each(
    [[], {}, [[], []], [[{ name: 4 }]], [[null]], [[{ name: "" }]]].map(
      (value) => ({ value }),
    ),
  )("rejects malformed REST label pages", ({ value }) =>
    expect(() => decodeLabelPages(value)).toThrow(),
  )

  test("last target read failure leaves every issue untouched", async () => {
    const { client, calls } = responses(
      new Map([[issuePath(9), { message: "API failure" }]]),
    )
    await expect(discoverPlan(client, repo, 12)).rejects.toThrow()
    expect(calls).toHaveLength(4)
    expect(calls.every((c) => !c.argv.includes("POST"))).toBe(true)
  })

  test("rejects malformed JSON without repeating response contents", async () => {
    const client = new GhClient(async () => "private unparseable response")
    await expect(discoverPlan(client, repo, 12)).rejects.toThrow(
      "malformed JSON",
    )
  })

  test("rejects incomplete and repeated GraphQL pagination", async () => {
    const incomplete = structuredClone(fixture.closing)
    incomplete.data.repository.pullRequest.closingIssuesReferences.pageInfo.hasNextPage =
      true
    expect(() => decodeClosingPage(incomplete)).toThrow("incomplete")
    incomplete.data.repository.pullRequest.closingIssuesReferences.nodes =
      Array.from({ length: 100 }, () => ({
        number: 4,
        repository: { nameWithOwner: repo },
      }))
    const client = new GhClient(async () => JSON.stringify(incomplete))
    await expect(discoverPlan(client, repo, 12)).rejects.toThrow(
      "repeated pagination",
    )
  })
})

describe("additive execution and rerun", () => {
  test("reports completed/pending on partial failure and safely reconciles a rerun", async () => {
    const existing = new Map<number, Set<string>>([
      [4, new Set(["S-triage", "A-ai"])],
      [9, new Set(["M-launch", "Z-mcp"])],
    ])
    const calls: Command[] = []
    let failSecondWrite = true
    const run: RunCommand = async (command) => {
      calls.push(command)
      if (command.argv.includes("graphql"))
        return JSON.stringify(fixture.closing)
      if (command.argv.includes(sourcePath))
        return JSON.stringify(fixture.sourceLabels)
      for (const [number, labels] of existing) {
        if (command.argv.includes(issuePath(number)))
          return JSON.stringify([[...labels].map((name) => ({ name }))])
        if (command.argv.includes(`repos/${repo}/issues/${number}/labels`)) {
          expect(command.argv).toContain("POST")
          expect(calls.slice(0, 4).every((c) => !c.argv.includes("POST"))).toBe(
            true,
          )
          if (number === 9 && failSecondWrite) throw new Error("Write failed")
          const payload = object(parseJson(command.input ?? ""), "write")
          if (!Array.isArray(payload.labels)) throw new Error("Missing labels")
          const names: readonly unknown[] = payload.labels
          for (const name of names) {
            if (typeof name !== "string") throw new Error("Invalid label")
            labels.add(name)
          }
          return JSON.stringify([...labels].map((name) => ({ name })))
        }
      }
      throw new Error("Unexpected request")
    }
    const github = new GhClient(run)
    const plan = await discoverPlan(github, repo, 12)
    try {
      await executePlan(github, plan)
      throw new Error("Expected partial failure")
    } catch (error) {
      expect(error).toBeInstanceOf(PartialWriteError)
      if (!(error instanceof PartialWriteError)) throw error
      expect(error.report.completed.map((a) => a.number)).toEqual([4])
      expect(error.report.pending.map((a) => a.number)).toEqual([9])
    }
    expect(existing.get(4)?.has("S-triage")).toBe(true)
    failSecondWrite = false
    const remaining = await discoverPlan(github, repo, 12)
    expect(remaining.additions.map((a) => a.number)).toEqual([9])
    await executePlan(github, remaining)
    expect((await discoverPlan(github, repo, 12)).additions).toEqual([])
  })

  test("shell metacharacters stay JSON data and never become command arguments", async () => {
    const name = "A-$(touch /tmp/do-not-run);'quoted'"
    const { client } = responses(new Map([[sourcePath, [[{ name }]]]]))
    const result = dryRun(await discoverPlan(client, repo, 12))
    for (const command of result.commands) {
      expect(command.argv.some((arg) => arg.includes("touch"))).toBe(false)
      expect(parseJson(command.input ?? "")).toEqual({ labels: [name] })
    }
  })

  test("CLI requires explicit repository/positive PR and defaults to dry-run", () => {
    expect(parseOptions(["--repo", repo, "--pr", "12"])).toEqual({
      repo,
      pr: 12,
      run: false,
    })
    expect(parseOptions(["--repo", repo, "--pr", "12", "--run"]).run).toBe(true)
    for (const args of [
      [],
      ["--repo", "$(bad)/repo", "--pr", "1"],
      ["--repo", repo, "--pr", "0"],
      ["--repo", repo, "--pr", "1x"],
    ]) {
      expect(() => parseOptions(args)).toThrow()
    }
  })
})
