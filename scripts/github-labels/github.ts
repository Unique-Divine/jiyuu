import {
  decodeClosingPage,
  decodeLabelPages,
  decodeWrittenLabels,
  parseJson,
  repository,
} from "./decode.ts"
import { additionCommand } from "./plan.ts"
import type { Addition, ClosingPage, Command, GitHub } from "./types.ts"

export type RunCommand = (command: Command) => Promise<string>

export const runCommand: RunCommand = async (command) => {
  const child = Bun.spawn([...command.argv], {
    stdin: command.input === null ? "ignore" : new Blob([command.input]),
    stdout: "pipe",
    stderr: "pipe",
  })
  const [stdout, , code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ])
  if (code !== 0)
    throw new Error(`GitHub command failed with exit code ${code}`)
  return stdout
}

const query = `query($owner:String!,$name:String!,$number:Int!,$cursor:String){
  repository(owner:$owner,name:$name){
    nameWithOwner
    pullRequest(number:$number){
      merged
      closingIssuesReferences(first:100,after:$cursor){
        nodes{number repository{nameWithOwner}}
        pageInfo{hasNextPage endCursor}
      }
    }
  }
}`

export class GhClient implements GitHub {
  constructor(private readonly run: RunCommand = runCommand) {}

  async closingPage(
    repo: string,
    pr: number,
    cursor: string | null,
  ): Promise<ClosingPage> {
    const [owner, name] = repository(repo).split("/")
    if (!owner || !name) throw new Error("Repository must be OWNER/REPO")
    const command: Command = {
      argv: ["gh", "api", "graphql", "--input", "-"],
      input: JSON.stringify({
        query,
        variables: { owner, name, number: pr, cursor },
      }),
    }
    return decodeClosingPage(parseJson(await this.run(command)))
  }

  async labels(repo: string, number: number): Promise<readonly string[]> {
    const command: Command = {
      argv: [
        "gh",
        "api",
        "--method",
        "GET",
        `repos/${repository(repo)}/issues/${number}/labels?per_page=100`,
        "--paginate",
        "--slurp",
      ],
      input: null,
    }
    return decodeLabelPages(parseJson(await this.run(command)))
  }

  async addLabels(addition: Addition): Promise<void> {
    // POST is additive. PUT would replace the issue's entire label set.
    const labels = decodeWrittenLabels(
      parseJson(await this.run(additionCommand(addition))),
    )
    if (!addition.labels.every((label) => labels.includes(label))) {
      throw new Error("GitHub did not confirm all requested labels")
    }
  }
}
