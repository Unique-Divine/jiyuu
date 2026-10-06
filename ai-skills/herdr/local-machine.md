# Herdr on this machine

Use these local sources when answering questions about Herdr configuration,
commands, or behavior. Prefer direct evidence over remembered defaults.

## Evidence order

1. For the user's effective configuration, read managed file
   `$DOTFILES/herdr-cfg/config.toml`. The file is intentionally
   self-documenting: comments preserve the meaning of available settings, and
   uncommented values are deliberate overrides. It is linked to runtime path
   `~/.config/herdr/config.toml`.
2. For defaults and command syntax supported by the installed binary, run
   command `herdr --default-config`, command `herdr --help`, or the relevant
   nonmutating command-group help.
3. For implementation details and fuller documentation, inspect source checkout
   `$DOTFILES/herdr`.
4. Reconcile versions before applying source-checkout findings to the installed
   binary. Draft source and documentation may describe unreleased behavior.

Reading files and printing help or the default config does not require
`HERDR_ENV=1`. Commands that inspect or control a live Herdr session do require
a Herdr-managed pane, as described in `SKILL.md`.

## Local paths

- Managed config:
  `$DOTFILES/herdr-cfg/config.toml`
- Local config notes:
  `$DOTFILES/herdr-cfg/README.md`
- Herdr source checkout:
  `$DOTFILES/herdr`
- Config data model and defaults:
  `$DOTFILES/herdr/src/config/model.rs`
- Commented default-config template:
  `$DOTFILES/herdr/src/main.rs`
- Unreleased English documentation:
  `$DOTFILES/herdr/docs/next/website/src/content/docs`

For guided config documentation, begin with file `configuration.mdx`. For the
generated key catalog, use file `config-reference.mdx` together with the config
model and command `herdr --default-config`.

## Configuration changes

When the user requests a local configuration change, edit managed file
`$DOTFILES/herdr-cfg/config.toml`, preserve its explanatory
comment style, and validate it with:

```bash
HERDR_CONFIG_PATH="$DOTFILES/herdr-cfg/config.toml" \
  herdr config check
```

Reloading the running server with command `herdr server reload-config` is a live
session-control action. Perform the `HERDR_ENV=1` check first.

## Installed fork and Codex

The vendored v0.9.3 fork keeps zero-based tab shortcuts and labels. Build it
with command `just herdr-install` from directory `$DOTFILES`. Command
`herdr update` replaces the custom binary with an upstream release. Check
command `herdr status server` separately from command `herdr --version`, since
installing a binary does not replace a running server.

The managed Codex defaults set configuration key `tui.alternate_screen` to
`"never"`. Codex reads this setting when its process starts and renders inline
so Herdr copy mode can scroll its output. Inline rendering works independently
of daemon mode.

Herdr's managed starts and restores, and the Zsh function `codex` for manual
launches, pass flag `--no-daemon` to keep session hooks tied to the launching
pane. See file `$DOTFILES/herdr-cfg/README.md` for applying these settings and
performing a server handoff.

Existing Codex processes survive live handoff with their original settings.
A shared Codex daemon can also retain another pane's inherited caller context.
When investigating that case, compare environment variable `CODEX_THREAD_ID` with live
API field `agent_session.value` before choosing a pane; do not infer identity from cwd
or recency alone. Use the verified explicit pane ID rather than a stale
`--current` context.

Do not edit the `herdr` source checkout merely to answer a configuration or
behavior question. Modify that checkout only when the user explicitly requests
Herdr source or documentation work.
