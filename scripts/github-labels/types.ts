export interface IssueRef {
  readonly repo: string
  readonly number: number
}

export interface ClosingPage {
  readonly repo: string
  readonly merged: boolean
  readonly issues: readonly IssueRef[]
  readonly nextCursor: string | null
}

export interface IssueLabels extends IssueRef {
  readonly labels: readonly string[]
}

export interface Addition extends IssueRef {
  readonly labels: readonly string[]
}

export interface LabelPlan {
  readonly repo: string
  readonly pr: number
  readonly sourceLabels: readonly string[]
  readonly skippedCrossRepository: number
  readonly additions: readonly Addition[]
}

export interface Command {
  readonly argv: readonly string[]
  readonly input: string | null
}

export interface GitHub {
  closingPage(
    repo: string,
    pr: number,
    cursor: string | null,
  ): Promise<ClosingPage>
  labels(repo: string, number: number): Promise<readonly string[]>
  addLabels(addition: Addition): Promise<void>
}

export interface ExecutionReport {
  readonly completed: readonly Addition[]
  readonly pending: readonly Addition[]
}
