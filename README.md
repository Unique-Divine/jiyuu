# Unique-Divine/自由 (jiyuu)

CLI utilities monorepo solving concrete dev problems: Go coverage merging, Windows↔Linux encoding fixes, language-agnostic dev commands, Markdown TOC generation, and LLM context parsing. Built primarily in Go with TypeScript components.

## ⚡jiyuu

| Path | Description |
| ---- | ----------- |
| 📂 [gocovmerge](./gocovmerge/README.md)     | Go coverage profile merger with modern CLI features |
| 📂 [mdtoc](./mdtoc/README.md) | Markdown table of contents (TOC) generator |
| 📂 [mycli](./mycli/README.md)               | Language-agnostic "ud" CLI tool for personal use |
| 📂 [winfixtext](./winfixtext/README.md)     | Fixes Windows encoding issues and corrupted LLM text outputs |
| 📂 [aictx](./aictx/)                        | CLI tool for parsing files into LLM-compatible context |
| 📂 [bash-ts](./bash-ts/README.md)           | Production TypeScript scripting library (bun install `@uniquedivine/bash`) for robust scripts with Bun runtime |
| 📂 [ud-jiyuu](./ud-jiyuu/README.md)         | TypeScript package mimicking Rust/Go functionality |
| 📂 [discord-nibiru](./discord-nibiru/README.md) | Discord moderation bot for Nibiru community |
| 📦 scripts | Scripts crate in Rust |
| ├── justfile | Runs project-specific commands |
| └── README.md  | |

## Hacking

Install `just` to run project-specific commands.

```bash
cargo install just
```

You can view the list of available development commands with `just -ls`.

Ref: [github.com/casey/just](https://github.com/casey/just)

## Work In Progress

Some tools are experimental or under active development:

- **aictx**: Not yet implemented - planned CLI for LLM context file parsing
- **mdtoc**: Functional but still has open TODOs in sprint list
