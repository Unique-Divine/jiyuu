# Install and Connect

## Install the Skill

Install `ai-skills/sai-deals` from [Jiyuu](https://github.com/Unique-Divine/jiyuu/tree/main/ai-skills/sai-deals).
Use the reviewed Jiyuu commit SHA as the source ref. Preserve the entire
directory so the workflow and example links resolve. On Linux and macOS,
Codex discovers the user skill under `~/.agents/skills/sai-deals`. Other agent
clients can use the same directory in their configured skill location.

For a manual Codex install, set `SAI_DEALS_SKILL_COMMIT` to the reviewed
40-character commit SHA. This command stages that commit's skill in a temporary
directory. Move an existing skill out of the destination before replacing it.

```bash
(
  set -eu
  : "${SAI_DEALS_SKILL_COMMIT:?Set the reviewed Jiyuu commit SHA}"
  destination="$HOME/.agents/skills/sai-deals"
  if [ -e "$destination" ] || [ -L "$destination" ]; then
    printf 'Skill location already exists: %s\n' "$destination" >&2
    exit 1
  fi
  stage="$(mktemp -d)"
  trap 'rm -rf -- "$stage"' EXIT
  git clone --quiet --no-checkout https://github.com/Unique-Divine/jiyuu.git "$stage/repo"
  git -C "$stage/repo" checkout --quiet --detach "$SAI_DEALS_SKILL_COMMIT"
  mkdir -p "$HOME/.agents/skills"
  cp -R "$stage/repo/ai-skills/sai-deals" "$destination"
)
```

## Choose a Reader

Use [Deal Desk GraphQL](graphql.md) with `curl` and `jq`. For a native CLI,
use a `sai-ops` build that supports reader mode. Check the published
[Jiyuu releases](https://github.com/Unique-Divine/jiyuu/releases) when the
`sai-ops/v0.1.0` release is available. Use the named release and its
`install-sai-ops.sh` and `SHA256SUMS` assets. The installer selects and
verifies the platform archive. From a directory containing the reviewed
installer, run `sh install-sai-ops.sh 0.1.0`, then check the command
with `sai-ops --version` and view reader commands with
`sai-ops --reader --help`. Use GraphQL if you have no verified reader
binary.

## Connect the Agent

Load the issued team key into the agent process as `SAI_DEAL_DESK_TOKEN`
through its secret store. The browser's Google sign-in and the Telegram
requester allowlist are separate access paths.

With the 1Password CLI, put the field's secret reference in a local env file.
For example, put this line in `deal-desk.env`, replacing the placeholders
with the actual reference.

```dotenv
SAI_DEAL_DESK_TOKEN=op://<vault>/<item>/<field>
```

Use `op run --env-file ./deal-desk.env -- <agent-command>` to start the agent.
For a terminal read, start a shell with
`op run --env-file ./deal-desk.env -- bash`, then run the appropriate check
inside that shell. Keep the resolved value out of the prompt and shell history.

- Native CLI. Run `sai-ops --reader health`.
- GraphQL. Run the [credential query](graphql.md#check-the-credential).

Use `--reader` on native commands to select reader mode explicitly.

Proceed when the check confirms the base permission `records:read` alone.
The issuer verifies workflow grants separately. A team key can see records
across Deal Desk, and the prompt's deal scope limits the task. On a missing,
invalid, or revoked key, ask the teammate to check the issued key in their
secret store.
