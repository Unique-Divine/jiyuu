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
