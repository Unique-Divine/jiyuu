package aictx

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/Unique-Divine/jiyuu/aictx/src/gitignore"
	cli "github.com/urfave/cli/v3"
)

func NewAppCmd() *cli.Command {
	return &cli.Command{
		Name:      "aictx",
		Usage:     "Combine files into a single LLM-friendly output",
		ArgsUsage: "<path> [path2 ...]",
		Flags: []cli.Flag{
			&cli.StringFlag{
				Name:    "output",
				Aliases: []string{"o"},
				Usage:   "Write result to file",
			},
			&cli.IntFlag{
				Name:        "level",
				Aliases:     []string{"L"},
				Usage:       `Limit the depth of recursion`,
				Required:    false,
				DefaultText: "-1",
				Value:       -1,
			},
			&cli.StringFlag{
				Name:  "lang",
				Usage: `Select default language ignore presets. Ex. "go", "ts", "py", "rust"].`,
			},
		},
		Action:   actionFunc,
		Commands: []*cli.Command{},
	}
}

var _ cli.ActionFunc = actionFunc

// actionFunc is the action to execute when no subcommands are specified
func actionFunc(goCtx context.Context, c *cli.Command) error {
	opts := new(FlagOpts)

	// Flag --level
	opts.Output = c.String("output")

	// Core Args: root paths to traverse with recursion.
	rawCmdArgs := c.Args().Slice()
	if len(rawCmdArgs) == 0 {
		_, err := c.ErrWriter.Write(
			[]byte("aictx requires at least one path (file, directory, or glob)\n\n"),
		)
		if err != nil {
			return err
		}
		return cli.ShowAppHelp(c)
	}

	// Flag --level
	opts.Level = c.Int("level")
	if opts.Level < 0 {
		opts.Level = -1
	}

	// TODO: impl lang flag feature for addition of ignore patterns.
	lang := c.String("lang")
	if len(lang) != 0 {
	}

	argPaths, err := ExpandGlobs(rawCmdArgs)
	if err != nil {
		return err
	}
	if len(argPaths) == 0 {
		return cli.Exit("no paths matched", 1)
	}

	rc := NewRunCtx()
	rc.Opts = opts

	// Load ignore lines from repo .gitignore based on cwd
	err = rc.LoadIgnoreLines()
	if err != nil {
		return err
	}

	// Add custom ignore patterns specified by user.
	var userIgnoreLines []string
	rc.BuildIgnorer(userIgnoreLines...)

	var buf bytes.Buffer
	rcBz, _ := json.MarshalIndent(rc, "", "  ")
	fmt.Fprintf(&buf, "\nRunCtx: %+s\n", rcBz)
	if err := stitchPaths(rc, &buf, argPaths); err != nil {
		return err
	}
	bufBz := buf.Bytes()

	if len(opts.Output) != 0 {
		if err := os.WriteFile(opts.Output, bufBz, 0o644); err != nil {
			return err
		}
	}

	// Print to stdout so you can see the stitched result in the terminal.
	_, err = c.Writer.Write(bufBz)
	return err
}

type FlagOpts struct {
	Level  int    // Limit the depth of recursion. -1 means unset.
	Output string // Output file to write. Uses stdout by default (output="").
}

// ExpandGlobs returns the collection of individual paths made by expanding any
// globs present in the paths given by "args".
func ExpandGlobs(args []string) ([]string, error) {
	var out []string
	for _, a := range args {
		matches, err := filepath.Glob(a)
		if err != nil {
			return nil, err
		}
		// If Glob finds matches, use them. Otherwise treat the arg as a literal path.
		if len(matches) > 0 {
			out = append(out, matches...)
		} else {
			out = append(out, a)
		}
	}
	return out, nil
}

func stitchPaths(rc *RunCtx, w io.Writer, paths []string) error {
	for _, p := range paths {
		absP, err := filepath.Abs(p)
		if err != nil {
			return err
		}
		if _, isSeen := rc.seenAbsPaths[absP]; isSeen {
			continue
		}

		if err := rc.stitchPath(w, p); err != nil {
			return err
		}

		rc.seenAbsPaths[absP] = struct{}{}
	}
	return nil
}

func (rc *RunCtx) stitchPath(w io.Writer, rootPath string) error {
	info, err := os.Stat(rootPath)
	if err != nil {
		return err
	}

	absRootP, err := filepath.Abs(rootPath)
	if err != nil {
		return err
	}
	// Skip processing the file if we've already done so.
	if _, isSeen := rc.seenAbsPaths[absRootP]; isSeen {
		return nil
	}
	rc.seenAbsPaths[absRootP] = struct{}{}

	if rc.ShouldIgnore(absRootP) {
		return nil
	}

	if info.IsDir() {
		maxRelDepth := -1 // maximum relative depth from rootPath
		if rc.Opts != nil && rc.Opts.Level >= 0 {
			maxRelDepth = rc.Opts.Level
		}

		return filepath.WalkDir(
			rootPath,
			func(path string, d os.DirEntry, err error) error {
				absP, err := filepath.Abs(path)
				if err != nil {
					return err
				}
				// Skip if we've already processed this path.
				if _, isSeen := rc.seenAbsPaths[absP]; isSeen {
					return nil
				}

				rc.seenAbsPaths[absP] = struct{}{}

				// Enforce max depth if set
				if maxRelDepth >= 0 {
					depth := depthWithinRoot(absRootP, absP)
					if depth > maxRelDepth {
						if d.IsDir() {
							return filepath.SkipDir // Prune this subtree
						}
						return nil // Skip this file.
					}
				}

				// Directory handling
				shouldIgnore := rc.ShouldIgnore(absP)
				if d.IsDir() {
					if shouldIgnore {
						return filepath.SkipDir
					}
					return nil
				}

				// File handling
				if shouldIgnore {
					return nil
				}
				return rc.stitchFile(w, path)
			},
		)
	}

	return rc.stitchFile(w, rootPath) // Case: If the root path is a file.
}

const (
	codeMarkerDepth4 string = "````"
)

func (rc *RunCtx) stitchFile(w io.Writer, path string) error {
	// Header format can be tuned for LLM prompts.
	if _, err := fmt.Fprintf(
		w,
		"\n\n### FILE: %s\n",
		path,
	); err != nil {
		return err
	}

	f, err := os.Open(path)
	if err != nil {
		return err
	}
	defer f.Close()

	_, _ = fmt.Fprintf(w, "\n%v%v\n", codeMarkerDepth4, LangForPath(f.Name()))
	_, err = io.Copy(w, f)
	_, _ = fmt.Fprintf(w, "%v\n", codeMarkerDepth4)
	return err
}

func FindGitRepo(path string) (isRepo bool, repoPath string, err error) {
	cmd := exec.Command("bash", "-c",
		strings.Join([]string{
			"cd " + path,
			"git rev-parse --show-toplevel",
		}, " && "),
	)
	bz, err := cmd.CombinedOutput()
	if err != nil {
		return false, "", fmt.Errorf("%v: %s", err, bz)
	}
	return true, string(bz), nil
}

// LoadIgnoreLines checks if the current working directory is a git repo and then
// returns the lines from its .gitignore file if one is present.
func (rc *RunCtx) LoadIgnoreLines() error {
	// Load ignore lines from CWD
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}

	ok, repoRoot, err := FindGitRepo(cwd)
	if ok && err == nil {
		repoRoot = strings.TrimSpace(repoRoot)
	}

	rc.Cwd = cwd
	rc.CwdRepoRoot = repoRoot
	giPath := filepath.Join(repoRoot, ".gitignore")
	if _, err := os.Stat(giPath); err == nil {
		bz, err := os.ReadFile(giPath)
		if err != nil {
			return err
		}
		rc.CwdRepoGitIgnore = giPath
		lines := strings.Split(string(bz), "\n")
		rc.ignoreLines = append(rc.ignoreLines, lines...)
	}

	// Global ignore lines

	return nil
}

type RunCtx struct {
	Cwd              string
	CwdRepoRoot      string
	CwdRepoGitIgnore string

	Opts         *FlagOpts
	ignorer      *gitignore.GitIgnore
	ignoreLines  []string
	seenAbsPaths map[string]struct{}
}

func NewRunCtx() *RunCtx {
	return &RunCtx{
		ignorer:      nil,
		seenAbsPaths: make(map[string]struct{}),
	}
}

// BuildIgnorer compiles rc.ignoreLines plus any extra lines (e.g., from flags)
// into a single GitIgnore object.
func (rc *RunCtx) BuildIgnorer(extraLines ...string) {
	all := make([]string, 0, len(rc.ignoreLines)+len(extraLines))
	all = append(all, rc.ignoreLines...)
	all = append(all, extraLines...)

	if len(all) == 0 {
		rc.ignorer = nil
		return
	}

	rc.ignorer = gitignore.CompileIgnoreLines(all...)
}

func (rc RunCtx) ShouldIgnore(absPath string) bool {
	if rc.ignorer == nil {
		return false
	}

	var (
		// Path to the root of the repo for the gitignore
		root = rc.CwdRepoRoot
		// Gitignore semantics expect relative paths based on teh git
		// repo that contains them. Absolue paths won't work.
		rel string
	)

	if root == "" {
		root = rc.Cwd // fallback if no repo
	}

	// Convert to a relative path so .gitignore rules work correctly.
	rel, err := filepath.Rel(root, absPath)
	if err != nil {
		// If something weird happens, just treat as not ignored.
		return false
	}

	// Gitignore library expects forward slashes. We have to call
	// filepath.ToSlash here to account for Windows backslashes.
	rel = filepath.ToSlash(rel)

	return rc.ignorer.MatchesPath(rel)
}

// depthWithinRoot returns how many path segments "path" is below "root".
// root and path should both be absolute paths.
func depthWithinRoot(root, path string) int {
	root = filepath.Clean(root)
	root = filepath.ToSlash(root)

	path = filepath.Clean(path)
	path = filepath.ToSlash(path)

	if root == path {
		return 0
	}

	rootCount := strings.Count(root, "/")
	pathCount := strings.Count(path, "/")
	return pathCount - rootCount
}
