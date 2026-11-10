# Testable Scripts

Child page of agent skill `ud-scripts`. These patterns come from the scripts
that have held up best: release publishers, installers, deploy helpers, and
label sync tools. Use the ones that fit. A ten-line helper needs none of
them; a script that publishes a release or writes to a remote usually wants
most of them.

[`scripts/github-labels`](https://github.com/Unique-Divine/jiyuu/tree/main/scripts/github-labels)
in the jiyuu repository shows the plan, runner, and test patterns together.

## Build a Plan, Then Print It or Run It

Split the script into a pure planner and an executor:

- The planner reads inputs (flags, files, API responses) and returns data:
  what will change and the exact commands, as argument lists. It never
  writes anything.
- The executor takes that plan and runs it.

Build the commands once and use the same list for both printing and
running. When the printed plan and the executed commands come from separate
code, they drift, and the printout stops telling the truth.

```ts
export interface Command { argv: string[]; input?: string }
export interface Plan { tag: string; commands: Command[] }

export function planRelease(sha: string, tags: string[]): Plan {
  const tag = nextTag(tags)
  return {
    tag,
    commands: [
      { argv: ["git", "tag", "-a", tag, sha, "-m", `Release ${tag}`] },
      { argv: ["git", "push", "origin", tag] },
    ],
  }
}
```

## Plan Mode and `--run`

For dangerous or one-off scripts, a pattern that works well: print the plan
by default and act only when `--run` is passed. A `--sim` mode that
simulates the change is another option. Neither is required. Pick what fits
how often the script runs and how much damage a mistake does.

If the script has both a plan mode and flags that only make sense when
running, it helps to reject contradictory combinations up front.

## Inject the Runner

Pass the thing that runs commands (or talks to an API, or downloads files)
into the executor instead of calling it directly:

```ts
type Runner = (cmd: Command) => Promise<string>

export async function publish(plan: Plan, run: Runner = spawnRunner) {
  for (const cmd of plan.commands) await run(cmd)
}
```

```py
def publish(plan, run=subprocess.run):
    for cmd in plan["commands"]:
        run(cmd["argv"], input=cmd.get("input"), text=True, check=True)
```

Tests then pass a fake runner that records each command, returns canned
output, and throws on any command it does not expect.

## Test Behavior, Not Internals

- **Compare exact command sequences.** Assert the full list of argv the
  fake runner received, in order, with `toEqual` in Bun or `assert ... ==`
  in pytest.
- **Assert what must not happen.** Zero calls when a precondition fails, no
  writes during discovery, no build step when the release already exists.
  These catch the bugs that hurt.
- **Use golden data.** Keep captured real tool output (a `gh` JSON
  response, a CLI listing) and the expected plan in fixture files, such as
  `fixtures/*.json`. When behavior changes on purpose, update the fixture
  and review the diff like code.
- **Start fixtures from real output.** Capture it from the live tool, then
  trim or anonymize it. Fixtures written from scratch test your assumptions,
  not the tool.

## Fake Tools on `PATH`

To test a Bash script, or a script that shells out to tools you can't
inject, put fake executables first on `PATH` in a temp directory. Each fake
records its arguments to a file and prints canned output or exits with a
chosen code. Name the temp directory with a space in it to catch quoting
bugs. Check exit codes too, for example 2 for bad usage and 127 for a
missing tool.

## Safety Around Real Changes

- **Preflight before any write.** Check the branch, a clean checkout, that
  the tag matches `HEAD`, free ports, or a new run directory, then start
  changing things.
- **Make reruns safe.** Skip work that is already done (the release exists,
  the cached artifact checks out). After a partial failure, report what
  finished and what is left, so a rerun picks up the rest.
- **Write atomically.** Stage next to the target, on the same filesystem,
  and `rename` into place; a rename across filesystems is a copy.
  Take a lock file opened exclusively when two runs could overlap. Keep the
  previous binary so a failed install can restore it.
- **Verify what you ship or install.** Check sha256 checksums and a
  metadata file that records the source revision, and confirm the source did
  not change during the build.
- **Leave a record** for operations worth auditing: a receipt file per
  phase, or a `commands.jsonl` log of every command run.
- **Keep stdout for data.** Print the plan or result on stdout and
  diagnostics on stderr, so another script can read the output.

## Run the Tests in CI

Script tests are cheap to run in CI on changes to the scripts, with a path
filter if the suite is slow. For scripts that publish, running the publish
step only from a tag or a protected branch, after the tests pass, has worked
well.
