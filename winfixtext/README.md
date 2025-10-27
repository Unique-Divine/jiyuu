# winfixtext

A Go command-line tool that fixes common Windows Unicode character encoding
issues in text files by replacing malformed characters with their standard ASCII
equivalents.

I work on a Windows machine on Ubuntu 24.04 via Windows Subsystem for Linux (WSL)
and often work with LLM tools like ChatGPT and Claude. These tools have a
tendency to spit out strange Unicode characters for apostrophes, quotation marks,
and other symbols. This tool essentially cleans up those outputs.

## Problem

When copying text from Windows applications (like Microsoft Word) or dealing with files that have encoding issues, you often end up with garbled Unicode characters instead of standard punctuation:

- `ΓÇÖ` or `╬ô├ç├û` instead of `'` (apostrophe)
- `ΓÇô` or `╬ô├ç├┤` instead of `-` (dash)
- `ΓÇ£` and `ΓÇ¥` instead of `"` and `"` (quotation marks)

## Solution

`winfixtext` processes files in-place, replacing these malformed characters with their correct ASCII equivalents.

## Installation

```bash
go install github.com/Unique-Divine/jiyuu/winfixtext@latest
```

Or build from source:
```bash
cd winfixtext
go build -o winfixtext main.go
```

## Usage

```bash
winfixtext [file]
```

The tool processes the specified file in-place, creating a temporary file during processing to ensure data safety.

### Examples

```bash
# Fix encoding issues in a document
winfixtext document.txt

# Fix a configuration file
winfixtext config.yaml

# Fix multiple files
for file in *.txt; do winfixtext "$file"; done
```

## Character Replacements

The tool performs these replacements:

| Malformed | → | Fixed |
|-----------|---|-------|
| `ΓÇÖ` | → | `'` |
| `╬ô├ç├û` | → | `'` |
| `'` | → | `'` |
| `ΓÇô` | → | `-` |
| `╬ô├ç├┤` | → | `-` |
| `"` | → | `"` |
| `"` | → | `"` |
| `ΓÇ£` | → | `"` |
| `ΓÇ¥` | → | `"` |

## Features

- **Safe in-place editing**: Uses temporary files and atomic rename operations
- **Line-by-line processing**: Memory efficient for large files
- **Newline normalization**: Ensures files end with newlines
- **Cross-platform**: Works on Windows, macOS, and Linux

## How It Works

1. Opens the input file for reading
2. Creates a temporary file in the same directory
3. Processes the input line by line, applying character replacements
4. Writes processed content to the temporary file
5. Atomically replaces the original file with the processed version

This approach ensures that if the process is interrupted, your original file remains intact.

## Exit Codes

- `0`: Success
- `1`: Invalid arguments or file access errors

## Common Use Cases

- Cleaning up documents copied from Microsoft Word
- Fixing encoding issues in configuration files
- Processing text files with Windows character encoding problems
- Batch processing files with shell loops

## Requirements

- Go 1.22.12 or later

## Testing

Run the test suite:
```bash
go test
```

The tests verify character replacement correctness and end-to-end file processing
functionality.

## License

BSD 2-Clause License (permissive). See [Unique-Divine/jiyuu/gocovmerge/LICENSE.md](https://github.com/Unique-Divine/jiyuu/blob/main/winfixtext/LICENSE.md)
