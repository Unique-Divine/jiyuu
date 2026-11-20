# Agent Automation

## Choose a Session

Use `photocraft-cli mcp` for an MCP client, `photocraft-cli serve` for a persistent JSON-lines session, and the desktop control channel when the task needs the running window. Check `photocraft-cli mcp --help` first: flags and default connection behavior change between releases.

The installed v0.23.1 release starts a headless MCP session with plain `mcp` and does **not** accept `--headless`. Newer source documentation describes `mcp --headless` and a plain `mcp` that can connect to or start the desktop app. Do not assume the source checkout and binary are the same version. For a headless session, follow the installed version's help and documentation.

For v0.23.1, this gives an MCP session access to one input directory and one output directory:

```sh
photocraft-cli mcp \
  --automation-read-root /absolute/path/to/input \
  --automation-write-root /absolute/path/to/output
```

The directories must exist. Read roots may be repeated and are searched in order; the write root is singular. Each direction is denied when its root is omitted. Choose roots appropriate to the task: these grants give the client access to files beneath them. A separate output folder limits accidental overwrites. A document opened through a different read root may need an explicit save path beneath the write root. Desktop root environment variables do not grant headless access.

## Discover and Drive

MCP tool `command_list` exposes command IDs and parameter descriptions; `command_run` executes one, and `doc_inspect` reports document state. Discover the available tool schemas from the connected server before invoking them. A `command_batch` is ordered but not atomic: successful earlier steps remain if a later step fails. Inspect the final document and render a preview when appearance matters.

The `serve` subcommand accepts JSON lines and retains a headless session. Its help lists the available methods. Standard input/output needs no control token; a TCP listener requires the documented control-token setup. Restrict roots the same way as for MCP.

## Control a Desktop Window

A live bridge requires a running desktop app with its control channel enabled. Configure read and write roots on the desktop process, then connect the CLI with `mcp --bridge` and the token file; root flags on the bridging CLI are rejected. UI inspection and screenshot tools require the live app. Read the installed release's `control-protocol.md` and `mcp.md`, or the matching upstream documentation, for the exact startup flags and methods. Do not expose the control token in output.
