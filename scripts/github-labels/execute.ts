import type { ExecutionReport, GitHub, LabelPlan } from "./types.ts"

export class PartialWriteError extends Error {
  constructor(readonly report: ExecutionReport) {
    super("Label write failed; rerun to discover remaining additions")
  }
}

export async function executePlan(
  github: GitHub,
  plan: LabelPlan,
): Promise<ExecutionReport> {
  const completed = []
  for (const [index, addition] of plan.additions.entries()) {
    try {
      await github.addLabels(addition)
      completed.push(addition)
    } catch {
      // A failed response may follow a successful write. Rerun discovery safely.
      throw new PartialWriteError({
        completed,
        pending: plan.additions.slice(index),
      })
    }
  }
  return { completed, pending: [] }
}
