# tmux-agent-watch

`tmux-agent-watch` observes visible terminal activity in Cursor CLI and Codex
CLI panes. It retains screen-change history in a small process and can append
compact summaries to tmux window names.

The prototype answers:

> Which known agent panes are visibly changing, and which have gone quiet?

It does not claim that a static pane is ready, done, blocked, or waiting for
input.

## Install and run

```bash
just test
just install
```

Recipe `just install` installs the release binary at stable path:

```text
~/.local/bin/tmux-agent-watch
```

The dotfiles configuration starts the installed binary when a tmux server
loads:

```tmux
run-shell -b 'if [ -x "$HOME/.local/bin/tmux-agent-watch" ]; then exec "$HOME/.local/bin/tmux-agent-watch" --socket-path #{q:socket_path} watch >/dev/null 2>&1; fi'
```

If the binary is absent, tmux starts normally and the summary remains blank.
The watcher takes an advisory lock keyed by tmux server PID. Reloading
`~/.tmux.conf` can launch another command, but the duplicate exits successfully
without clearing or replacing the active watcher's state. Separate tmux servers
receive separate watcher processes.

For manual development, run command `just run`. Query the active watcher with:

```bash
tmux-agent-watch list
tmux-agent-watch status
tmux-agent-watch inspect %25
```

Use option `--socket NAME` for a server created with `tmux -L NAME`, or option
`--socket-path PATH` for an exact server created with `tmux -S PATH`.

The default observation interval is 500 ms. A pane becomes `static` after
2,500 ms without a rendered-screen change:

```bash
cargo run -- watch --interval-ms 500 --static-after-ms 2500
```

Use `--no-tmux-status` to write snapshots without changing tmux display
options.

To stop or restart a watcher manually, first inspect its PID:

```bash
pgrep -af 'tmux-agent-watch.*watch'
kill <pid>
tmux source-file ~/.tmux.conf
```

The watcher exits normally when its original tmux server disappears.

## State and display

The watcher discovers panes through command `tmux list-panes`, recognizes
foreground commands `agent` and `codex`, and samples command
`tmux capture-pane`. Captured bytes remain in process memory only.

The watcher publishes user option `@agent_watch_summary` on each tmux window:

```text
│ ↻↻ .
```

- Each `↻` represents one visibly changing agent pane.
- Each `.` represents one screen-static agent pane.

The compact display has two states only. It does not track visit history or an
acknowledgment state.

The label is blank by default. Configure one when starting the watcher:

```bash
tmux-agent-watch watch --tmux-label AGENTS --tmux-separator " | "
tmux-agent-watch watch --changing-symbol "●" --static-symbol "."
```

The dotfiles integration appends format `#{@agent_watch_summary}` to the
existing tmux window labels.

## Snapshot

The watcher writes a versioned JSON snapshot atomically to:

```text
$XDG_RUNTIME_DIR/tmux-agent-watch/server-<tmux-pid>.json
```

If `XDG_RUNTIME_DIR` is unavailable, it uses:

```text
/tmp/tmux-agent-watch-$USER/server-<tmux-pid>.json
```

The directory uses mode `0700`, and the snapshot uses mode `0600`. The
snapshot contains structural pane metadata and derived activity state. It does
not contain pane captures, prompts, full process command lines, environment
variables, or conversation text.

Pane titles, window names, and session names can still reveal project or task
names. The snapshot is local private runtime data, not a payload that is safe
to publish without an explicit filtering and authentication design.

On graceful shutdown, the watcher clears its tmux window summaries if the
original server still owns the socket. It also clears old summaries when a new
watcher starts. A forced kill or process crash releases the kernel lock but can
leave stale tmux user options until the watcher restarts.

## Output boundary

The observer publishes one versioned snapshot that presentation sinks can
consume. The tmux user-option writer and terminal commands are the implemented
sinks. A future Neovim view, local dashboard, or authenticated remote publisher
should consume the same structural state rather than repeat terminal capture.

Snapshots represent latest state. A future transition event stream would serve
notifications. Neither interface should publish pane captures; remote output
also needs filtering for titles and names, authentication, and host/server
identity.

## Known gaps

- A visually static agent may still be waiting on a model, network request, or
  quiet subprocess.
- Spinners, statuslines, human typing, editor redraw, and pane resizing can
  produce visible changes that are not autonomous agent work.
- Focus behavior has only been characterized for one attached tmux client.
- tmux pane IDs identify panes only within the lifetime of one tmux server.
- The prototype has no network listener, remote publisher, orchestration, or
  input-control API.
