# tg-sai

Sai Telegram admin audit and bootstrap CLI.

See file `RUNBOOK.md` for operator setup, profile management, and safe
execution workflows.

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
