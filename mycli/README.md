# Unique-Divine/mycli

Implements the "ud" command-line tool, which helps lower my cognitive load by
aggregating some of my most commonly used commands into one place. I follow
language-agnostic patterns where possible.

For example, rather than remembering how to format in Go and Rust separately or
run tests, it's easier for me to think in patterns of what I want to get done:

```bash
ud go test  # Test in Go
ud rs test  # Test in Rust

ud go fmt   # Formats with gofumpt
ud rs fmt   # Formats with rustfmt
```

## What this tool is not

1. It's not intended for use by others: Many settings and commands are
   personalized for my machine and my dotfiles. If your setup and OS mirror mine
   (Neovim, WSL, Ubuntu), this app might be useful for you too. I imagine it will
   only be useful as a learning aid in writing your own CLI in either Go or Rust.
2. It's not general purpose: There's likely a better way to write this
   application to be more future-proof or make fewer assumptions about which
   commands are available. To avoid premature optimization, I've ignored the urge to
   optimize here and stuck purely to a utility-first approach, only implementing
   what seemed maxmially useful for making me develop other software faster.

## Project Brain Dump

- [ ] Implement a minimal working app in Rust with similar behavior to the
      current Go version. This would be done purely as a learning exercise to expand
      my skills.
- [ ] Add LLM shortcuts for prompts or other common API interactions.
