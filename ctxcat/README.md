# ctxcat

File to context converter for passing files to feed LLMs. Combines multiple files into a single, well-formatted context with proper syntax highlighting.

## Usage

```bash
# Process current directory
ctxcat .

# Process specific files
ctxcat main.go src/lib.rs README.md

# Use glob patterns
ctxcat "src/*.go" "**/*.md"

# Limit recursion depth
ctxcat --level 2 .

# Output to file
ctxcat --output context.md src/
```

## Key Concepts

**Recursive Processing**: By default, `ctxcat` walks entire directory trees. Use `--level` to limit depth.

**Gitignore Respect**: Automatically finds and applies `.gitignore` rules from your repo root. No need to manually exclude `node_modules`, `target/`, etc.

**Smart Formatting**: Each file gets a clear header and appropriate syntax highlighting based on file extension.

**Deduplication**: Won't process the same file twice, even if specified multiple times or found via different paths.

## Examples

```bash
# Get full project context for an LLM
ctxcat . --output full-context.md

# Just the source code, skip deep nesting
ctxcat src/ --level 3

# Specific file patterns across the project
ctxcat "*.go" "*.md" "Dockerfile*"

# Multiple directories with different purposes
ctxcat src/ docs/ scripts/ --output project-overview.md
```

## Installation

```bash
go install github.com/Unique-Divine/jiyuu/ctxcat@latest

# Or build from source
cd jiyuu/ctxcat && just install
```

## Flags

- `-o, --output FILE` - Write to file instead of stdout
- `-L, --level N` - Limit recursion depth (default: unlimited)

```bash
# Save context to a file for later use
ctxcat . -o project-context.md

# Only traverse 2 levels deep in directories
ctxcat src/ --level 2

# Combine both: shallow traversal saved to file
ctxcat . --level 1 --output overview.md
```
