# aictx

File to context converter for passing files to feed LLMs. Combines multiple files into a single, well-formatted context with proper syntax highlighting.

## Usage

```bash
# Process current directory
aictx .

# Process specific files
aictx main.go src/lib.rs README.md

# Use glob patterns
aictx "src/*.go" "**/*.md"

# Limit recursion depth
aictx --level 2 .

# Output to file
aictx --output context.md src/
```

## Key Concepts

**Recursive Processing**: By default, `aictx` walks entire directory trees. Use `--level` to limit depth.

**Gitignore Respect**: Automatically finds and applies `.gitignore` rules from your repo root. No need to manually exclude `node_modules`, `target/`, etc.

**Smart Formatting**: Each file gets a clear header and appropriate syntax highlighting based on file extension.

**Deduplication**: Won't process the same file twice, even if specified multiple times or found via different paths.

## Examples

```bash
# Get full project context for an LLM
aictx . --output full-context.md

# Just the source code, skip deep nesting
aictx src/ --level 3

# Specific file patterns across the project
aictx "*.go" "*.md" "Dockerfile*"

# Multiple directories with different purposes
aictx src/ docs/ scripts/ --output project-overview.md
```

## Installation

```bash
go install github.com/Unique-Divine/jiyuu/aictx@latest

# Or build from source
cd jiyuu/aictx && just install
```

## Flags

- `-o, --output FILE` - Write to file instead of stdout
- `-L, --level N` - Limit recursion depth (default: unlimited)

```bash
# Save context to a file for later use
aictx . -o project-context.md

# Only traverse 2 levels deep in directories
aictx src/ --level 2

# Combine both: shallow traversal saved to file
aictx . --level 1 --output overview.md
```
