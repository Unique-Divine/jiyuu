# GitHub Conventions

Labels describe work across issues, pull requests, and GitHub Projects. Shared
area names let related work in different repositories appear in the same view.
Zones identify code within this repository. A zone name in another repository
does not imply the same paths.

## Label Families

| Prefix | Meaning | How It Is Assigned |
| --- | --- | --- |
| `A-` | Product or engineering area | Issue triage or PR paths where mapped |
| `Z-` | Repository code zone | Issue triage or PR paths where mapped |
| `M-` | Mission or milestone | Human or agent classification |
| `S-` | Issue status | Human or agent triage; never inherited from PRs |
| `prio-p` | Priority | Human or agent triage; never inherited from PRs |

The shared vocabulary lives in [agent skill gh-labels](ai-skills/gh-labels/SKILL.md).
Read this file before assigning repository-specific labels. Labels can overlap;
a Telegram MCP change receives the AI area, the MCP parent zone, and its Telegram
child zone.

## Code Checks

Code checks live in [the shared test workflow](.github/workflows/test.yaml).
Its `changes` job detects affected projects once. Downstream jobs use those
outputs in step-level `if` conditions, including checkout and tool setup.
Unaffected jobs print a reason and succeed. Keep the `test` and `mdtoc-test`
check names because branch protection requires them.

Add a project's paths and shared dependencies to that job's filters when adding
checks. Changes to the workflow or root Just recipes run every code check;
manual dispatch also runs everything. Pull requests and pushes to main or release
branches use the path filters. Keep workflow-level path restrictions out of this
workflow so every check reports a result.

Python type errors and warnings fail CI, with native diagnostics and summaries
uploaded as artifacts even when findings fail the check. The report recipes use
explicit `enforce` mode in CI; their default `report` mode remains available for
investigation. Environment setup, configuration errors, and checker failures
fail in both modes. Label validation and privileged issue/label
automation have separate workflows.

## Path-Managed Labels

The table describes Jiyuu's [PR mappings](.github/labeler.yml). CI checks that
these names and path lists agree. Area labels use color `eab6cb`; zone labels
use `61afef`.

| Label | Description | Paths |
| --- | --- | --- |
| `A-ai` | Area: AI tooling, MCP servers, agent skills, and agent workflow tools | `mcp/**`, `ai-skills/**`, `tmux-agent-watch/**` |
| `Z-mcp` | Zone: MCP servers and shared MCP support | `mcp/**` |
| `Z-mcp-discord` | Zone: Discord MCP server | `mcp/discord/**` |
| `Z-mcp-slack` | Zone: Slack MCP server | `mcp/slack/**` |
| `Z-mcp-telegram` | Zone: Telegram MCP server | `mcp/telegram/**` |
| `Z-ai-skills` | Zone: Agent skills | `ai-skills/**` |
| `Z-tmux-agent-watch` | Zone: tmux agent observer | `tmux-agent-watch/**` |
| `Z-ci` | Zone: GitHub automation and Just recipes | `.github/**`, `scripts/github-labels/**`, `justfile`, `**/justfile`, `*.just`, `**/*.just` |

Files under `ts-pkg/tg-sai/**` belong to the separate Telegram CLI. They do not
match the MCP Telegram zone. Other tools such as `gh-rev` and `ctxcat` do not
acquire a new path mapping merely because an agent uses them.

The PR labeler adds and removes only the labels in its mapping as changed paths
evolve. Other labels retain their human or agent assignments. Maintain the live
label names, descriptions, and colors with `gh label create --force` when
changing this table; the PR workflow applies existing labels.

## Closing Issue Inheritance

After a PR merges, CI first synchronizes its path labels, then adds its eligible
classification labels to issues GitHub identifies as closed by that PR.
Eligible names match `^[A-Z]-.+$`, except `S-` status labels. This includes `A-`,
`Z-`, `M-`, and future uppercase families. Priority and ordinary lowercase labels
do not pass through.

Inheritance adds missing labels and never removes issue labels. It applies
only to closing issues in the same repository. Cross-repository references are
counted and skipped before reading their labels; their titles and URLs are not
logged. A mixed PR passes its classification labels to every same-repository
closing issue. Keep closing references specific enough for that behavior.

Preview or replay the operation from the repository root:

```sh
just github-labels --repo Unique-Divine/jiyuu --pr 123
just github-labels --repo Unique-Divine/jiyuu --pr 123 --run
```

The default command prints a sorted plan and exact argument arrays and JSON
stdin for the additive GitHub API calls. `--run` executes that plan only after
all discovery reads succeed. On a partial failure it exits nonzero and reports
completed and pending issues. Rerun discovery to reconcile an uncertain write.
The workflow's manual dispatch accepts a merged PR number for the same replay.

The privileged workflow reads default-branch code and configuration. It never
checks out a PR branch. Code and fixture tests run in a separate read-only PR
workflow. Automation uses the repository token, without a personal token.

## Issue Triage and Historical Backfill

Agents label issues using their actual scope, code paths, and linked changes.
Do not guess from a title containing "Telegram" or "AI." Historical paths may
have moved. Read every page of changed files before classifying a large PR.

For history, add classifications backed by the issue text or changed paths;
leave ambiguous items unchanged. Preserve status, priority, and existing
classifications. A mixed PR is evidence to inspect, not proof that every label
belongs on every linked historical issue. Private planning can reference public
work; public issues, PRs, and logs must not quote private ticket content.

Private and public repositories can share names such as `A-ai` for Projects
grouping without copying private text or granting cross-repository automation
access. Broader repository mappings remain explicit local decisions.
