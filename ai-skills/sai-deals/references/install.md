# Install and Connect

## Install the Skill

Install the [public `sai-deals` skill](https://github.com/Unique-Divine/jiyuu/tree/main/ai-skills/sai-deals)
from Jiyuu's `main` branch. Give your agent's skill installer the directory
link, or copy the complete `ai-skills/sai-deals` directory into the agent's
skills directory. The workflow and example files belong with `SKILL.md`.

On Linux and macOS, this command installs the skill for Codex under
`~/.agents/skills/sai-deals`. Move an existing installation aside before
replacing it.

```bash
(
  set -eu
  destination="$HOME/.agents/skills/sai-deals"
  if [ -e "$destination" ] || [ -L "$destination" ]; then
    printf 'Skill location already exists: %s\n' "$destination" >&2
    exit 1
  fi
  stage="$(mktemp -d)"
  trap 'rm -rf -- "$stage"' EXIT
  git clone --quiet --depth 1 --branch main \
    https://github.com/Unique-Divine/jiyuu.git "$stage/repo"
  mkdir -p "$HOME/.agents/skills"
  cp -R "$stage/repo/ai-skills/sai-deals" "$destination"
)
```

## Choose a Reader

Use [Deal Desk GraphQL](graphql.md) with `curl` and `jq`.
To install the `sai-ops` CLI on Linux or macOS, download
[`install-sai-ops.sh`](https://github.com/Unique-Divine/jiyuu/releases/download/sai-ops/v0.1.0/install-sai-ops.sh)
from the [Sai Ops release](https://github.com/Unique-Divine/jiyuu/releases/tag/sai-ops/v0.1.0).
Run these commands from the download directory. The installer fetches
the release's checksum manifest and verifies the archive for your platform.

```sh
sh ./install-sai-ops.sh 0.1.0
export PATH="$HOME/.local/bin:$PATH"
sai-ops --version
sai-ops --reader --help
```

Add `~/.local/bin` to your shell startup file to keep `sai-ops` on `PATH`
in future sessions.

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
