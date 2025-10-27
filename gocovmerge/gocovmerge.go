// gocovmerge: merge multiple `go test -coverprofile` files into one.
//
// Build:   go build -o gocovmerge
// Install: go install ./...
//
// Usage:
//
//	gocovmerge [-o merged.out] [--] <profile1> <profile2> ...
//	gocovmerge -o coverage.total.out coverage.group1.out coverage.group2.out
//	gocovmerge -o total.out coverage.*.out
//	gocovmerge -o - -                 # read from stdin, write to stdout
//
// Notes:
// - All input files must share the same coverage mode (set|count|atomic).
// - Blocks are merged per file; counts are OR'ed for "set", added for "count"/"atomic".
package main

import (
	"bufio"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/tools/cover"
)

const version = "v0.2.0"

var (
	flagOut   = flag.String("o", "", "output file (default stdout)")
	flagQuiet = flag.Bool("q", false, "quiet mode (suppress non-error output)")
)

func usage() {
	fmt.Fprintf(flag.CommandLine.Output(),
		`gocovmerge %s - merge multiple Go coverage profiles

Usage:
  gocovmerge [-q] [-o merged.out] [--] <profile1> <profile2> ...
  gocovmerge -o coverage.total.out coverage.group1.out coverage.group2.out
  gocovmerge -o total.out coverage.*.out
  gocovmerge -o - -                 # read from stdin, write to stdout

Options:
  -o string   Output file. "-" means stdout. If empty, stdout is used.
  -q          Quiet mode (print only errors)

Input:
  - Each argument may be a file path, "-" (stdin), or a glob pattern.
  - All inputs must use the same coverage mode (set, count, or atomic).

Exit codes:
  0 on success, 1 on usage error, 2 on merge error.

Examples:
  gocovmerge -o coverage.total.out coverage.group*.out
  gocovmerge -o coverage.out coverage.unit.out coverage.integration.out
`, version)
}

func main() {
	log.SetFlags(0)
	flag.Usage = usage
	flag.Parse()

	files, err := expandArgs(flag.Args())
	if err != nil {
		failUsage(err)
	}
	if len(files) == 0 {
		failUsage(errors.New("no input profiles provided"))
	}

	var out io.Writer = os.Stdout
	if *flagOut != "" && *flagOut != "-" {
		f, err := os.Create(*flagOut)
		if err != nil {
			failMerge(fmt.Errorf("open output %q: %w", *flagOut, err))
		}
		defer func() {
			_ = f.Close()
		}()
		out = f
	}

	merged, mode, err := mergeAll(files)
	if err != nil {
		failMerge(err)
	}

	if err := dumpProfiles(merged, mode, out); err != nil {
		failMerge(err)
	}

	if !*flagQuiet {
		// A small confirmation line to stderr so stdout stays clean when piped
		fmt.Fprintf(os.Stderr, "merged %d profiles (%s)\n", len(files), mode)
	}
}

func expandArgs(args []string) ([]string, error) {
	var out []string
	for _, a := range args {
		if a == "-" {
			out = append(out, "-")
			continue
		}
		// Accept comma-separated lists to make CI calls convenient.
		for _, part := range strings.Split(a, ",") {
			part = strings.TrimSpace(part)
			if part == "" {
				continue
			}
			if hasGlob(part) {
				matches, _ := filepath.Glob(part)
				out = append(out, matches...)
				continue
			}
			out = append(out, part)
		}
	}
	// De-dup while preserving order.
	seen := make(map[string]struct{}, len(out))
	dst := out[:0]
	for _, f := range out {
		if _, ok := seen[f]; ok {
			continue
		}
		seen[f] = struct{}{}
		dst = append(dst, f)
	}
	return dst, nil
}

func hasGlob(s string) bool {
	return strings.ContainsAny(s, "*?[")
}

func mergeAll(files []string) ([]*cover.Profile, string, error) {
	var (
		merged []*cover.Profile // kept sorted by FileName
		mode   string
	)
	for _, path := range files {
		var r io.Reader
		var srcName string

		if path == "-" {
			r = bufio.NewReader(os.Stdin)
			srcName = "stdin"
		} else {
			f, err := os.Open(path)
			if err != nil {
				return nil, "", fmt.Errorf("open %q: %w", path, err)
			}
			defer f.Close()
			r = f
			srcName = path
		}

		// ParseProfiles requires a filename; for stdin we slurp into a temp file-like buffer.
		// Workaround: read all then call ParseProfiles on a temp file on disk or emulate header parse.
		// Easier: if not a real path, write to a temp file.
		tmpPath := ""
		if path == "-" {
			b, err := io.ReadAll(r)
			if err != nil {
				return nil, "", fmt.Errorf("read stdin: %w", err)
			}
			tf, err := os.CreateTemp("", "gocovmerge-stdin-*.out")
			if err != nil {
				return nil, "", fmt.Errorf("stdin tempfile: %w", err)
			}
			tmpPath = tf.Name()
			if _, err = tf.Write(b); err != nil {
				tf.Close()
				return nil, "", fmt.Errorf("stdin tempfile write: %w", err)
			}
			tf.Close()
			defer os.Remove(tmpPath)
			path = tmpPath
		}

		profiles, err := cover.ParseProfiles(path)
		if err != nil {
			return nil, "", fmt.Errorf("parse %s: %w", srcName, err)
		}
		if len(profiles) == 0 {
			continue
		}
		if mode == "" {
			mode = profiles[0].Mode
		}
		if profiles[0].Mode != mode {
			return nil, "", fmt.Errorf("cannot merge profiles with different modes: have %q, got %q from %s", mode, profiles[0].Mode, srcName)
		}
		for _, p := range profiles {
			merged = addProfile(merged, p)
		}
	}

	// Ensure deterministic file order.
	sort.Slice(merged, func(i, j int) bool { return merged[i].FileName < merged[j].FileName })
	return merged, mode, nil
}

func addProfile(profiles []*cover.Profile, p *cover.Profile) []*cover.Profile {
	i := sort.Search(len(profiles), func(i int) bool { return profiles[i].FileName >= p.FileName })
	if i < len(profiles) && profiles[i].FileName == p.FileName {
		mergeProfiles(profiles[i], p)
		return profiles
	}
	profiles = append(profiles, nil)
	copy(profiles[i+1:], profiles[i:])
	profiles[i] = p
	return profiles
}

func mergeProfiles(dst, src *cover.Profile) {
	if dst.Mode != src.Mode {
		log.Fatalf("cannot merge profiles with different modes: %q vs %q", dst.Mode, src.Mode)
	}
	start := 0
	for _, b := range src.Blocks {
		start = mergeProfileBlock(dst, b, start)
	}
}

func mergeProfileBlock(p *cover.Profile, pb cover.ProfileBlock, start int) int {
	// Binary search in p.Blocks[start:] for first block >= pb (by start line/col).
	find := func(i int) bool {
		pi := p.Blocks[i+start]
		return pi.StartLine > pb.StartLine || (pi.StartLine == pb.StartLine && pi.StartCol >= pb.StartCol)
	}
	i := 0
	if !find(i) {
		i = sort.Search(len(p.Blocks)-start, find)
	}
	i += start
	// Same block?
	if i < len(p.Blocks) && p.Blocks[i].StartLine == pb.StartLine && p.Blocks[i].StartCol == pb.StartCol {
		if p.Blocks[i].EndLine != pb.EndLine || p.Blocks[i].EndCol != pb.EndCol {
			log.Fatalf("overlap merge in %s: %v vs %v", p.FileName, p.Blocks[i], pb)
		}
		switch p.Mode {
		case "set":
			p.Blocks[i].Count |= pb.Count
		case "count", "atomic":
			p.Blocks[i].Count += pb.Count
		default:
			log.Fatalf("unsupported covermode: %q", p.Mode)
		}
		return i + 1
	}

	// Sanity: make sure we’re not inserting inside an existing block.
	if i > 0 {
		prev := p.Blocks[i-1]
		if prev.EndLine > pb.EndLine || (prev.EndLine == pb.EndLine && prev.EndCol > pb.EndCol) {
			log.Fatalf("overlap before in %s: %v vs %v", p.FileName, prev, pb)
		}
	}
	if i < len(p.Blocks) {
		next := p.Blocks[i]
		if next.StartLine < pb.StartLine || (next.StartLine == pb.StartLine && next.StartCol < pb.StartCol) {
			log.Fatalf("overlap after in %s: %v vs %v", p.FileName, next, pb)
		}
	}

	p.Blocks = append(p.Blocks, cover.ProfileBlock{})
	copy(p.Blocks[i+1:], p.Blocks[i:])
	p.Blocks[i] = pb
	return i + 1
}

func dumpProfiles(profiles []*cover.Profile, mode string, out io.Writer) error {
	if len(profiles) == 0 {
		// Still emit a mode header; some tools expect it
		if _, err := fmt.Fprintf(out, "mode: %s\n", modeOrDefault(mode)); err != nil {
			return err
		}
		return nil
	}
	if _, err := fmt.Fprintf(out, "mode: %s\n", profiles[0].Mode); err != nil {
		return err
	}
	for _, p := range profiles {
		for _, b := range p.Blocks {
			if _, err := fmt.Fprintf(out, "%s:%d.%d,%d.%d %d %d\n",
				p.FileName, b.StartLine, b.StartCol, b.EndLine, b.EndCol, b.NumStmt, b.Count); err != nil {
				return err
			}
		}
	}
	return nil
}

func modeOrDefault(m string) string {
	if m == "" {
		return "atomic"
	}
	return m
}

func failUsage(err error) {
	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n\n", err)
	}
	usage()
	os.Exit(1)
}

func failMerge(err error) {
	fmt.Fprintf(os.Stderr, "merge error: %v\n", err)
	os.Exit(2)
}
