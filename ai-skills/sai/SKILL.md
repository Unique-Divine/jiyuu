---
name: sai
description: >-
  Trade and inspect Sai perpetuals on Nibiru from a terminal or agent with the
  open-source `sai` CLI and its MCP server. Use for Sai markets, prices,
  positions, balances, trade history, live price or position watches,
  connecting a Sai account, previewing or submitting trades, checking intents,
  or setting up the Sai MCP server.
---

# Sai CLI

Sai is a perpetual futures exchange on Nibiru. The `sai` CLI from
[SaiDotFun/open-sai](https://github.com/SaiDotFun/open-sai) reads markets and
accounts, streams updates, submits trading intents, and runs an MCP server.

## Install

Check for the CLI with `sai --version`. If it is missing, install it:

```bash
uv tool install git+https://github.com/SaiDotFun/open-sai
```

To try it without installing, run it through `uvx`:

```bash
uvx --from git+https://github.com/SaiDotFun/open-sai sai --json market prices
```

## Load the Packaged Skill

Run `sai --skill` and follow its output. It prints the skill shipped with the
installed CLI, so its commands and safety rules match that version. Prefer it
over this file and over any other copy of the `sai` skill. Use `sai --help`
and each command's `--help` for exact options.
