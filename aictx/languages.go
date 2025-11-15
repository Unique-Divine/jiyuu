package main

import (
	"path/filepath"
	"strings"
)

// extToLang maps file extensions (without leading ".") to a Markdown
// code fence language identifier.
//
// The values should match what common highlighters (GitHub, VS Code, etc.)
// expect after the ``` marker.
var extToLang = map[string]string{
	// Go
	"go":  "go",
	"mod": "go", // go.mod
	// You can leave go.sum as plain text or map to "go" too if you prefer.
	"sum": "",

	// JavaScript / TypeScript
	"js":  "javascript",
	"mjs": "javascript",
	"cjs": "javascript",
	"jsx": "jsx",
	"ts":  "typescript",
	"tsx": "tsx",

	// Python / Ruby / PHP
	"py":  "python",
	"rb":  "ruby",
	"php": "php",

	// Rust / C / C++ / C#
	"rs":  "rust",
	"c":   "c",
	"h":   "c",
	"cpp": "cpp",
	"cc":  "cpp",
	"cxx": "cpp",
	"hpp": "cpp",
	"cs":  "csharp",

	// Java / Kotlin / Scala
	"java":  "java",
	"kt":    "kotlin",
	"kts":   "kotlin",
	"scala": "scala",

	// Shell / CLI
	"sh":   "bash",
	"bash": "bash",
	"zsh":  "bash",
	"fish": "fish",
	"ps1":  "powershell",

	// Web
	"html":   "html",
	"htm":    "html",
	"css":    "css",
	"vue":    "vue",
	"svelte": "svelte",

	// Data / config
	"json": "json",
	"yml":  "yaml",
	"yaml": "yaml",
	"toml": "toml",
	"ini":  "ini",
	"env":  "", // usually key=value env files; plain text is fine

	// SQL
	"sql": "sql",

	// Markdown / text
	"md":       "markdown",
	"markdown": "markdown",
	"txt":      "",

	// Docker / containers
	"dockerfile":    "docker",
	"containerfile": "docker",

	// Make / build
	"makefile": "make",

	// Solidity / web3
	"sol": "solidity",

	// Proto / gRPC
	"proto": "protobuf",

	// Misc
	"yaml.tmpl": "yaml",
}

// LangForPath returns a language identifier suitable for use in a Markdown
// code fence based on the file extension of path.
//
// Example:
//
//	```go
//	lang := LangForPath("main.go") // "go"
//	```
//
// If no mapping is found, it returns the empty string, meaning you should
// emit an untagged code fence.
func LangForPath(path string) string {
	path = strings.ToLower(path)
	if lang, ok := extToLang[path]; ok {
		return lang
	}

	// Behavior for `filepath.Ext`
	// foo.go -> ".go"
	// README -> ""
	ext := filepath.Ext(path) // e.g. ".go"
	if ext == "" {
		return ""
	}
	ext = ext[1:] // "go"
	if lang, ok := extToLang[ext]; ok {
		return lang
	}
	return ""
}
