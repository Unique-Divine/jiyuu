import type {
  Addition,
  Command,
  GitHub,
  IssueLabels,
  LabelPlan,
} from "./types.ts"

export function eligibleLabel(name: string): boolean {
  return /^[A-Z]-.+$/.test(name) && !name.startsWith("S-")
}

function sortedUnique(values: readonly string[]): string[] {
  return [...new Set(values)].sort()
}

export function planLabels(
  repo: string,
  pr: number,
  source: readonly string[],
  issues: readonly IssueLabels[],
  skippedCrossRepository: number,
): LabelPlan {
  const sourceLabels = sortedUnique(source.filter(eligibleLabel))
  const additions = issues
    .map((issue) => ({
      repo,
      number: issue.number,
      labels: sourceLabels.filter((name) => !issue.labels.includes(name)),
    }))
    .filter((addition) => addition.labels.length > 0)
    .sort((a, b) => a.number - b.number)
  return { repo, pr, sourceLabels, skippedCrossRepository, additions }
}

/** Complete and validate discovery before producing any write operations. */
export async function discoverPlan(
  github: GitHub,
  repo: string,
  pr: number,
): Promise<LabelPlan> {
  let cursor: string | null = null
  let canonicalRepo = repo
  const cursors = new Set<string>()
  const targets = new Map<number, string>()
  const skipped = new Set<string>()
  do {
    const page = await github.closingPage(repo, pr, cursor)
    if (page.repo.toLowerCase() !== repo.toLowerCase()) {
      throw new Error("GitHub returned a different source repository")
    }
    canonicalRepo = page.repo
    if (!page.merged)
      throw new Error("Pull request is not merged; no labels were written")
    for (const issue of page.issues) {
      if (issue.repo.toLowerCase() !== canonicalRepo.toLowerCase()) {
        // Never fetch titles, labels, or URLs from another repository.
        skipped.add(`${issue.repo.toLowerCase()}#${issue.number}`)
        continue
      }
      targets.set(issue.number, canonicalRepo)
    }
    cursor = page.nextCursor
    if (cursor !== null) {
      if (cursors.has(cursor))
        throw new Error("GitHub returned a repeated pagination cursor")
      cursors.add(cursor)
    }
  } while (cursor !== null)

  const source = await github.labels(canonicalRepo, pr)
  const issues: IssueLabels[] = []
  for (const number of [...targets.keys()].sort((a, b) => a - b)) {
    issues.push({
      repo: canonicalRepo,
      number,
      labels: await github.labels(canonicalRepo, number),
    })
  }
  return planLabels(canonicalRepo, pr, source, issues, skipped.size)
}

export function additionCommand(addition: Addition): Command {
  return {
    argv: [
      "gh",
      "api",
      "--method",
      "POST",
      `repos/${addition.repo}/issues/${addition.number}/labels`,
      "--input",
      "-",
    ],
    input: JSON.stringify({ labels: addition.labels }),
  }
}

export function dryRun(plan: LabelPlan): {
  mode: "dry-run"
  plan: LabelPlan
  commands: Command[]
} {
  return {
    mode: "dry-run",
    plan,
    commands: plan.additions.map(additionCommand),
  }
}
