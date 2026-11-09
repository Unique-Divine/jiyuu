import { issueNumber, repository } from "./decode.ts"
import { executePlan, PartialWriteError } from "./execute.ts"
import { GhClient } from "./github.ts"
import { discoverPlan, dryRun } from "./plan.ts"

export interface Options {
  readonly repo: string
  readonly pr: number
  readonly run: boolean
}

export function parseOptions(args: readonly string[]): Options {
  let repo: string | undefined
  let pr: number | undefined
  let run = false
  for (let index = 0; index < args.length; index++) {
    const option = args[index]
    if (option === "--run" && !run) run = true
    else if (option === "--repo" && repo === undefined)
      repo = repository(args[++index])
    else if (option === "--pr" && pr === undefined) {
      const number = args[++index]
      if (!number || !/^[1-9]\d*$/.test(number))
        throw new Error("--pr requires a positive integer")
      pr = issueNumber(Number(number))
    } else
      throw new Error(
        "Usage: just github-labels --repo OWNER/REPO --pr NUMBER [--run]",
      )
  }
  if (!repo || pr === undefined) {
    throw new Error(
      "Usage: just github-labels --repo OWNER/REPO --pr NUMBER [--run]",
    )
  }
  return { repo, pr, run }
}

export async function main(args: readonly string[]): Promise<void> {
  const options = parseOptions(args)
  const github = new GhClient()
  const plan = await discoverPlan(github, options.repo, options.pr)
  if (!options.run) {
    console.log(JSON.stringify(dryRun(plan), null, 2))
    return
  }
  console.log(
    JSON.stringify(
      { mode: "run", plan, report: await executePlan(github, plan) },
      null,
      2,
    ),
  )
}

if (import.meta.main) {
  try {
    await main(Bun.argv.slice(2))
  } catch (error) {
    if (error instanceof PartialWriteError)
      console.error(JSON.stringify(error.report, null, 2))
    console.error(
      error instanceof Error ? error.message : "GitHub label operation failed",
    )
    process.exitCode = 1
  }
}
