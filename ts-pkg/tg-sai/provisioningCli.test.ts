import { expect, test } from "bun:test"
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

async function cliFixture(
  operation: (
    run: (
      args: string[],
    ) => Promise<{ exit: number; stdout: string; stderr: string }>,
    calls: string,
    requestFile: string,
  ) => Promise<void>,
) {
  const root = await mkdtemp(join(tmpdir(), "tg-provision-cli-"))
  const calls = join(root, "calls.jsonl"),
    requestFile = join(root, "request.json")
  const request = {
    requestId: "cli-test",
    custodyProfile: "missing-custody",
    expectedUserId: "10",
    title: "Sai test",
    participants: ["21"],
    admins: [],
    botRightsProfile: "baseline",
    dealIds: ["deal-a", "deal-b"],
  }
  try {
    await writeFile(requestFile, JSON.stringify(request))
    await writeFile(
      join(root, "sai-deal-desk"),
      `#!/usr/bin/env bun
import {appendFileSync} from "node:fs";
const input=JSON.parse(await Bun.stdin.text());
appendFileSync(process.env.TEST_CLOUD_CALLS, JSON.stringify({args:process.argv.slice(2),input})+"\\n");
console.log(JSON.stringify({...${JSON.stringify(request)},state:"prepared",chatId:null,claimToken:"private-test-token",resolvedParticipants:null,resolvedAdmins:null}));
`,
    )
    await chmod(join(root, "sai-deal-desk"), 0o700)
    const run = async (args: string[]) => {
      const child = Bun.spawn(
        [
          process.execPath,
          join(import.meta.dir, "main.ts"),
          "chat",
          "provision",
          ...args,
        ],
        {
          env: {
            ...process.env,
            TG_SAI_DIR: join(root, "profiles"),
            PATH: `${root}:${process.env.PATH}`,
            TEST_CLOUD_CALLS: calls,
          },
          stdout: "pipe",
          stderr: "pipe",
        },
      )
      const [stdout, stderr, exit] = await Promise.all([
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
        child.exited,
      ])
      return { stdout, stderr, exit }
    }
    await operation(run, calls, requestFile)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
}
test("prepare defaults to local dry-run without invoking cloud", async () =>
  cliFixture(async (run, calls, file) => {
    const result = await run(["prepare", "--file", file])
    expect(result.exit).toBe(0)
    expect(JSON.parse(result.stdout).dryRun).toBe(true)
    expect(await Bun.file(calls).exists()).toBe(false)
  }))
test("prepare --run calls only the authoritative cloud prepare with typed input", async () =>
  cliFixture(async (run, calls, file) => {
    const result = await run(["prepare", "--file", file, "--run"])
    expect(result.exit).toBe(0)
    const call = JSON.parse((await readFile(calls, "utf8")).trim())
    expect(call.args.slice(0, 3)).toEqual([
      "chat-setups",
      "prepare",
      "--json-stdin",
    ])
    expect(call.args).toContain("--request-key")
    expect(call.input.expectedUserId).toBe("10")
    expect(result.stdout).not.toContain("private-test-token")
  }))
test("run defaults to cloud view only, without local session setup or claim", async () =>
  cliFixture(async (run, calls) => {
    const result = await run(["run", "--request-id", "cli-test"])
    expect(result.exit).toBe(0)
    expect(JSON.parse(result.stdout).dryRun).toBe(true)
    const records = (await readFile(calls, "utf8"))
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line))
    expect(records).toHaveLength(1)
    expect(records[0].args[1]).toBe("view")
    expect(result.stdout).not.toContain("private-test-token")
  }))
test("run --run rejects missing custody profile before claim or any Telegram connection", async () =>
  cliFixture(async (run, calls) => {
    const result = await run(["run", "--request-id", "cli-test", "--run"])
    expect(result.exit).toBe(1)
    expect(result.stderr).toContain("Custody profile is not configured")
    const records = (await readFile(calls, "utf8"))
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line))
    expect(records).toHaveLength(1)
    expect(records[0].args[1]).toBe("view")
  }))
test("rewrite-description dry run needs the custody profile and only views the setup", async () =>
  cliFixture(async (run, calls) => {
    for (const args of [
      ["rewrite-description", "--request-id", "cli-test"],
      ["rewrite-description", "--request-id", "cli-test", "--run"],
    ]) {
      const result = await run(args)
      expect(result.exit).toBe(1)
      expect(result.stderr).toContain("Custody profile is not configured")
      expect(result.stdout).not.toContain("private-test-token")
    }
    const records = (await readFile(calls, "utf8"))
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line))
    expect(records.map((record) => record.args[1])).toEqual(["view", "view"])
  }))
