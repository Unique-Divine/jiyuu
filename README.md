# Unique-Divine/自由 (jiyuu)

(1) Rigorous notes and solutions for core algorithms and computing fundamentals broken down for deliberate practice with Anki. (2) A monorepo of libraries and CLI tools I use regularly as a software engineer.

## ⚡jiyuu

| Path | Description |
| ---- | ----------- |
| <span style="white-space: nowrap;">📂 [ai-skills](./ai-skills/)</span> | Cursor/LLM agent skills for recurring workflows, repo navigation, PRs, chain queries, and data lookups |
| <span style="white-space: nowrap;">📂 [algos](./algos/algos-notes.md)</span> | Algorithm practice problems with Go implementations and Anki-formatted notes for deliberate practice |
| <span style="white-space: nowrap;">📂 [ctxcat](./ctxcat/README.md)</span> | File to context convertor for pasing files to feed LLMs. Similar to `cat` and `bat`. |
| <span style="white-space: nowrap;">📂 [gocovmerge](./gocovmerge/README.md)</span> | Go coverage profile merger with modern CLI features |
| <span style="white-space: nowrap;">📂 [mdtoc](./mdtoc/README.md)</span> | Markdown table of contents (TOC) generator |
| <span style="white-space: nowrap;">📂 [winfixtext](./winfixtext/README.md)</span> | Fixes Windows encoding issues and corrupted LLM text outputs |
| <span style="white-space: nowrap;">📂 [ts-pkg/bash](./ts-pkg/bash/README.md)</span> | Production TypeScript scripting library (bun install `@uniquedivine/bash`) for robust scripts with Bun runtime |
| <span style="white-space: nowrap;">📂 [ts-pkg/jiyuu](./ts-pkg/jiyuu/README.md)</span> | TypeScript package mimicking Rust/Go functionality |
| <span style="white-space: nowrap;">📂 [discord-nibiru](./discord-nibiru/README.md)</span> | Discord moderation bot for Nibiru community |
| <span style="white-space: nowrap;">📦 scripts</span> | Scripts crate in Rust |
| <span style="white-space: nowrap;">├── justfile</span> | Runs project-specific commands |
<!-- | <span style="white-space: nowrap;">└── README.md</span>  | ... | -->

## `ai-skills`

`ai-skills/` contains reusable agent skills for Cursor/LLM workflows. Each
skill is a small, task-focused directory with a `SKILL.md` entrypoint and, when
needed, supporting docs like `reference.md`, `REFERENCE.md`, `examples.md`, or
schema notes.

Current skills cover workflows such as:

- writing commit messages and PR summaries
- routing work across related repos
- querying Nibiru via `nibid` or EVM RPC
- querying indexer, Sai GraphQL, REST, and Postgres-backed data sources
- checking governance proposals and upgrade status

In practice, this directory is the repo's library of operational playbooks for
repeatable AI-assisted tasks.

## Hacking

Install `just` to run project-specific commands.

```bash
cargo install just
```

You can view the list of available development commands with `just -ls`.

Ref: [github.com/casey/just](https://github.com/casey/just)
