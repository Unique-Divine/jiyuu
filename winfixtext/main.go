package main

import (
	"bufio"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
)

func main() {
	log.SetFlags(0)   // no timestamp
	log.SetPrefix("") // no prefixe
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "Incorrect number of args received.\nUsage: winfixtext [file]")
		os.Exit(1)
	}
	if err := FixFileInPlace(os.Args[1]); err != nil {
		log.Fatal(err)
	}
}

var (
	REPLACER = strings.NewReplacer(
		// Apostrophe replacements
		"╬ô├ç├û", `'`,
		"ΓÇÖ", `'`,
		"’", `'`,

		// Dash replacements
		`ΓÇô`, `-`,
		`╬ô├ç├┤`, `-`,

		// Quotation mark replacements
		`“`, `"`,
		`”`, `"`,
		`ΓÇ£`, `"`,
		`ΓÇ¥`, `"`,
	)
)

// ProcessStream reads line-by-line from r, applies replacements, and writes
// lines to w. It always writes a newline after each logical line (including the
// last, even if input lacked it).
func ProcessStream(r io.Reader, w io.Writer) error {
	repl := REPLACER
	reader := bufio.NewReader(r)
	writer := bufio.NewWriter(w)
	for {
		line, err := reader.ReadString('\n')
		switch err {
		case nil:
			line = strings.TrimSuffix(line, "\n")
			if _, werr := writer.WriteString(repl.Replace(line) + "\n"); werr != nil {
				return werr
			}
		case io.EOF:
			if len(line) > 0 { // final line without trailing newline
				line = strings.TrimSuffix(line, "\n")
				if _, werr := writer.WriteString(repl.Replace(line) + "\n"); werr != nil {
					return werr
				}
			}
			if err := writer.Flush(); err != nil {
				return err
			}
			return nil
		default:
			return fmt.Errorf("read error: %w", err)
		}
	}
}

// FixFileInPlace processes file at path using repl, writing through a temp file and renaming.
func FixFileInPlace(path string) error {
	in, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("open: %w", err)
	}
	defer in.Close()

	dir := filepath.Dir(path)
	base := filepath.Base(path)
	tmp, err := os.CreateTemp(dir, base+".*.tmp")
	if err != nil {
		return fmt.Errorf("create temp: %w", err)
	}
	tmpName := tmp.Name()

	// Ensure cleanup on any failure
	cleanup := func(e error) error {
		tmp.Close()
		_ = os.Remove(tmpName)
		return e
	}

	if err := ProcessStream(in, tmp); err != nil {
		return cleanup(err)
	}
	if err := tmp.Sync(); err != nil {
		return cleanup(err)
	}
	if err := tmp.Close(); err != nil {
		return cleanup(err)
	}
	// Close input before rename (important on Windows)
	if err := in.Close(); err != nil {
		return cleanup(err)
	}
	if err := os.Rename(tmpName, path); err != nil {
		return cleanup(fmt.Errorf("rename: %w", err))
	}
	return nil
}
