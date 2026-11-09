import type { ClosingPage, IssueRef } from "./types.ts"

function invalid(context: string): never {
  throw new Error(`Invalid GitHub response: ${context}`)
}

export function object(
  value: unknown,
  context: string,
): Record<string, unknown> {
  if (!isObject(value)) {
    return invalid(context)
  }
  return value
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function array(value: unknown, context: string): unknown[] {
  if (!Array.isArray(value)) return invalid(context)
  return value
}

function text(value: unknown, context: string): string {
  if (typeof value !== "string" || value.length === 0) return invalid(context)
  return value
}

export function repository(value: unknown): string {
  const name = text(value, "repository name")
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_.-]*\/[a-zA-Z0-9_.-]+$/.test(name)) {
    return invalid("repository name")
  }
  return name
}

export function issueNumber(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 1) {
    return invalid("issue number")
  }
  return value
}

function repoFrom(value: unknown): string {
  return repository(object(value, "repository").nameWithOwner)
}

export function decodeClosingPage(value: unknown): ClosingPage {
  const root = object(value, "GraphQL response")
  if (
    root.errors !== undefined &&
    array(root.errors, "GraphQL errors").length > 0
  ) {
    throw new Error("GitHub GraphQL returned errors; no labels were written")
  }
  const repo = object(
    object(root.data, "GraphQL data").repository,
    "repository",
  )
  const pr = object(repo.pullRequest, "pull request")
  if (typeof pr.merged !== "boolean") return invalid("merged state")
  const connection = object(
    pr.closingIssuesReferences,
    "closing issues connection",
  )
  const info = object(connection.pageInfo, "closing issues page info")
  if (typeof info.hasNextPage !== "boolean") return invalid("hasNextPage")
  if (info.endCursor !== null && typeof info.endCursor !== "string") {
    return invalid("endCursor")
  }
  const issues: IssueRef[] = array(connection.nodes, "closing issue nodes").map(
    (node) => {
      const issue = object(node, "closing issue")
      return {
        repo: repoFrom(issue.repository),
        number: issueNumber(issue.number),
      }
    },
  )
  if (issues.length > 100) return invalid("closing issue page size")
  if (info.hasNextPage && (issues.length !== 100 || !info.endCursor)) {
    return invalid("incomplete closing issue page")
  }
  return {
    repo: repoFrom(repo),
    merged: pr.merged,
    issues,
    nextCursor: info.hasNextPage ? text(info.endCursor, "next cursor") : null,
  }
}

/** gh api --paginate --slurp returns one array for each REST page. */
export function decodeLabelPages(value: unknown): string[] {
  const pages = array(value, "label pages")
  if (pages.length === 0) return invalid("missing label page")
  const labels: string[] = []
  for (const [index, page] of pages.entries()) {
    const rows = array(page, "label page")
    if (
      rows.length > 100 ||
      (index < pages.length - 1 && rows.length !== 100)
    ) {
      return invalid("incomplete label page")
    }
    for (const row of rows)
      labels.push(text(object(row, "label").name, "label name"))
  }
  return labels
}

/** A label-write response contains the full label list, not a REST page. */
export function decodeWrittenLabels(value: unknown): string[] {
  return array(value, "written labels").map((row) =>
    text(object(row, "label").name, "label name"),
  )
}

export function parseJson(value: string): unknown {
  try {
    const parsed: unknown = JSON.parse(value)
    return parsed
  } catch {
    throw new Error(
      "GitHub returned malformed JSON; no response content was logged",
    )
  }
}
