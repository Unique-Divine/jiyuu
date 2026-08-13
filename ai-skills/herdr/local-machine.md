# Herdr on this machine

Use these local sources when answering questions about Herdr configuration,
commands, or behavior. Prefer direct evidence over remembered defaults.

## Evidence order

1. For the user's effective configuration, read managed file
   `/home/realu/ki/boku/dotfiles/herdr/config.toml`. The file is intentionally
   self-documenting: comments preserve the meaning of available settings, and
   uncommented values are deliberate overrides. It is linked to runtime path
   `~/.config/herdr/config.toml`.
2. For defaults and command syntax supported by the installed binary, run
   command `herdr --default-config`, command `herdr --help`, or the relevant
   nonmutating command-group help.
3. For implementation details and fuller documentation, inspect source checkout
   `/home/realu/ki/boku/dotfiles/lib-herdr`.
4. Reconcile versions before applying source-checkout findings to the installed
   binary. Draft source and documentation may describe unreleased behavior.

Reading files and printing help or the default config does not require
`HERDR_ENV=1`. Commands that inspect or control a live Herdr session do require
a Herdr-managed pane, as described in `SKILL.md`.

## Local paths

- Managed config:
  `/home/realu/ki/boku/dotfiles/herdr/config.toml`
- Local config notes:
  `/home/realu/ki/boku/dotfiles/herdr/README.md`
- Herdr source checkout:
  `/home/realu/ki/boku/dotfiles/lib-herdr`
- Config data model and defaults:
  `/home/realu/ki/boku/dotfiles/lib-herdr/src/config/model.rs`
- Commented default-config template:
  `/home/realu/ki/boku/dotfiles/lib-herdr/src/main.rs`
- Unreleased English documentation:
  `/home/realu/ki/boku/dotfiles/lib-herdr/docs/next/website/src/content/docs`
- Published version documentation:
  `/home/realu/ki/boku/dotfiles/lib-herdr/docs/versions`

For guided config documentation, begin with file `configuration.mdx`. For the
generated key catalog, use file `config-reference.mdx` together with the config
model and command `herdr --default-config`.

## Configuration changes

When the user requests a local configuration change, edit managed file
`/home/realu/ki/boku/dotfiles/herdr/config.toml`, preserve its explanatory
comment style, and validate it with:

```bash
HERDR_CONFIG_PATH="/home/realu/ki/boku/dotfiles/herdr/config.toml" \
  herdr config check
```

Reloading the running server with command `herdr server reload-config` is a live
session-control action. Perform the `HERDR_ENV=1` check first.

Do not edit the `lib-herdr` source checkout merely to answer a configuration or
behavior question. Modify that checkout only when the user explicitly requests
Herdr source or documentation work.
