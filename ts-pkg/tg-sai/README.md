# tg-sai

CLI for Sai's Telegram operations: audit chats, move chats into the custody
account, run the custody worker that creates deal chats, and inspect what
every Telegram call did. `RUNBOOK.md` covers operator setup, profiles, and the
VM worker in depth.

## Install

Requires [Bun](https://bun.sh) 1.2 or newer.

```bash
bunx @uniquedivine/tg-sai --help        # run without installing
bun add -g @uniquedivine/tg-sai         # or install the `tg-sai` command
tg-sai profile add --name me --login --qr --interactive
```

Any command or command group run without arguments prints its help, for
example `tg-sai events` or `tg-sai holds`.

## Move Chats to the Custody Account

`ops sai-bootstrap` makes the Sai custody account,
[@SaiTeam_AI](https://t.me/SaiTeam_AI) (user 8274304175), the owner of chats
you own and adds [@SaiTeam_bot](https://t.me/SaiTeam_bot) as admin. It is a
dry run until you add `--run`:

```bash
tg-sai ops sai-bootstrap --file chats.txt          # plan only
tg-sai ops sai-bootstrap --file chats.txt --run    # apply
```

`chats.txt` holds one chat ID per line. Ownership transfer needs your Telegram
2FA password in `TELEGRAM_PASSWORD` or the saved profile. `--target-user`
picks a different owner.

## Telegram Rate Limits

Telegram does not report an account's limits ahead of time. It answers an
over-limit request with a wait (`FLOOD_WAIT`), which can grow to hours if
requests keep coming. tg-sai saves waits on adding members and creating
chats, plus any other wait too long to sleep through, in
`~/.tg-sai/state.db`. No tg-sai process sends a held request until the wait
ends, including after a restart or upgrade.

- A command blocked by a saved wait sends nothing, exits with code 75, and
  prints when to retry.
- `ops sai-bootstrap` stops the batch at the first long wait instead of
  failing every remaining chat.
- The custody worker leaves held chat requests queued. Requesters see
  "Queued … starts automatically around <time UTC>" in `/chatstatus`.

Inspect or extend saved waits with `tg-sai holds`. `set` and `clear` are dry
runs until you add `--run`; `set` can only lengthen a wait, and clearing an
active one needs `--force`:

```bash
tg-sai holds list
tg-sai holds set --account 8274304175 --group invite --wait-s 3600 --run
```

While a wait is active, avoid adding members from the custody account in the
Telegram app: it can extend the wait, and tg-sai cannot see app actions.
Limits are per account, so your own account is unaffected.

## Logs

tg-sai writes no log files. Each run records its Telegram calls, retries,
flood waits, and app log rows in `~/.tg-sai/telemetry.db`. Read them with
`tg-sai events`:

```bash
tg-sai events runs                      # recent commands and worker runs
tg-sai events show <run_id|job_id|request_id>
tg-sai events floods                    # waits Telegram returned
tg-sai events logs --since 24h --level warn
```

Stored rows keep ids, counts, and error codes, never passwords, sessions,
phone numbers, usernames, or message text. `TG_SAI_TELEMETRY=off` turns off
`telemetry.db` only; saved waits still apply. `TG_SAI_DIR` moves both files
out of `~/.tg-sai`.

## Development

```bash
bun install
just validate       # lint, tests, typecheck, npm dry run
uv run pytest session_string_generator_test.py
bun main.ts --help  # run from source
```

Tests use a throwaway `TG_SAI_DIR`. Publish with `just publish`.
