# tg-sai

Sai Telegram admin audit and bootstrap CLI.

See file `RUNBOOK.md` for operator setup, profile management, and safe
execution workflows.

## Install

Requires [Bun](https://bun.sh). No clone needed:

```bash
bunx @uniquedivine/tg-sai --help          # run once
bun add -g @uniquedivine/tg-sai           # or install the `tg-sai` command
tg-sai profile add --name me --login --qr --interactive
```

Move Sai chats you own to the custody account and add SaiTeam_bot as admin.
The command is a dry run until you add `--run`:

```bash
tg-sai ops sai-bootstrap --file chats.txt          # plan only
tg-sai ops sai-bootstrap --file chats.txt --run    # apply
```

File `chats.txt` holds one chat ID per line. Ownership transfer needs your
Telegram 2FA password in `TELEGRAM_PASSWORD` or the saved profile. The owner
target defaults to the Sai custody account,
[@SaiTeam_AI](https://t.me/SaiTeam_AI) (user 8274304175), and the bot is
[@SaiTeam_bot](https://t.me/SaiTeam_bot). `--target-user` overrides the owner.

## Logs

tg-sai writes no log files. Every run records Telegram calls and app log
rows in the SQLite file `~/.tg-sai/telemetry.db` (`$TG_SAI_DIR` overrides
`~/.tg-sai`). Read them with `tg-sai events`:

```bash
tg-sai events logs --since 24h --level warn --subsystem worker
tg-sai events show <run_id|job_id|request_id>
tg-sai events runs
```

Saved Telegram flood waits live in `~/.tg-sai/state.db`. While one is
active, a CLI command that would send the held call exits with code 75 and
prints when to retry. `TG_SAI_TELEMETRY=off` turns off `telemetry.db` only;
`state.db` and its holds still apply.

## Development

```bash
bun install
bun test
bun run typecheck
uv run pytest session_string_generator_test.py
```

Run the CLI from this package directory:

```bash
bun main.ts --help
```
