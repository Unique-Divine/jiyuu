# Unique-Divine/自由 (jiyuu)

(1) Rigorous notes and solutions for core algorithms and computing fundamentals broken down for deliberate practice with Anki. (2) A monorepo of libraries and CLI tools I use regularly as a software engineer.

## ⚡jiyuu

| Path | Description |
| ---- | ----------- |
| <span style="white-space: nowrap;">📂 [algos](./algos/algos-notes.md)</span> | Algorithm practice problems with Go implementations and Anki-formatted notes for deliberate practice |
| <span style="white-space: nowrap;">📂 [aictx](./aictx/README.md)</span> | File to context convertor for pasing files to feed LLMs. Similar to `cat` and `bat`. |
| <span style="white-space: nowrap;">📂 [gocovmerge](./gocovmerge/README.md)</span> | Go coverage profile merger with modern CLI features |
| <span style="white-space: nowrap;">📂 [mdtoc](./mdtoc/README.md)</span> | Markdown table of contents (TOC) generator |
| <span style="white-space: nowrap;">📂 [winfixtext](./winfixtext/README.md)</span> | Fixes Windows encoding issues and corrupted LLM text outputs |
| <span style="white-space: nowrap;">📂 [bash-ts](./bash-ts/README.md)</span> | Production TypeScript scripting library (bun install `@uniquedivine/bash`) for robust scripts with Bun runtime |
| <span style="white-space: nowrap;">📂 [ud-jiyuu](./ud-jiyuu/README.md)</span> | TypeScript package mimicking Rust/Go functionality |
| <span style="white-space: nowrap;">📂 [discord-nibiru](./discord-nibiru/README.md)</span> | Discord moderation bot for Nibiru community |
| <span style="white-space: nowrap;">📦 scripts</span> | Scripts crate in Rust |
| <span style="white-space: nowrap;">├── justfile</span> | Runs project-specific commands |
<!-- | <span style="white-space: nowrap;">└── README.md</span>  | ... | -->

## Hacking

Install `just` to run project-specific commands.

```bash
cargo install just
```

You can view the list of available development commands with `just -ls`.

Ref: [github.com/casey/just](https://github.com/casey/just)
