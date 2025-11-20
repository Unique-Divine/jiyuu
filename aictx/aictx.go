package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/Unique-Divine/jiyuu/aictx/gitignore"
	cli "github.com/urfave/cli/v2"
)

func main() {
	cliApp := &cli.App{
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
		Action: runCliApp,
		Commands: []*cli.Command{
			{
				Name:      "cat",
				Usage:     "stitch one or more paths (files, directories, or globs)",
				ArgsUsage: "<path> [path2 ...]",
				Flags: []cli.Flag{
					&cli.StringFlag{
						Name:    "output",
						Aliases: []string{"o"},
						Usage:   "write result to file in addition to copying it to the clipboard",
					},
				},
				Action: runCliApp,
			},
		},
	}

	if err := cliApp.Run(os.Args); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

func runCliApp(c *cli.Context) error {
	opts := new(FlagOpts)
	opts.Output = c.String("output")
	rawCmdArgs := c.Args().Slice()
	if len(rawCmdArgs) == 0 {
		_, err := os.Stderr.WriteString("aictx requires at least one path (file, directory, or glob)\n\n")
		if err != nil {
			return err
		}
		err = cli.ShowAppHelp(c)
		if err != nil {
			return err
		}
		return cli.Exit("", 1)
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
	data := buf.Bytes()

	if len(opts.Output) != 0 {
		if err := os.WriteFile(opts.Output, data, 0o644); err != nil {
			return err
		}
	}

	// Print to stdout so you can see the stitched result in the terminal.
	_, err = os.Stdout.Write(data)
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

func (rc *RunCtx) stitchPath(w io.Writer, pathRoot string) error {
	info, err := os.Stat(pathRoot)
	if err != nil {
		return err
	}

	absP, err := filepath.Abs(pathRoot)
	if err != nil {
		return err
	}
	// Skip processing the file if we've already done so.
	if _, isSeen := rc.seenAbsPaths[absP]; isSeen {
		return nil
	}
	if rc.ShouldIgnore(absP) {
		return nil
	}

	if info.IsDir() {
		return filepath.WalkDir(
			pathRoot,
			func(path string, d os.DirEntry, err error) error {
				absP, err := filepath.Abs(path)
				if err != nil {
					return err
				}
				// Skip processing the file if we've already done so.
				if _, isSeen := rc.seenAbsPaths[absP]; isSeen {
					return nil
				}

				if d.IsDir() {
					return nil
				}

				rc.seenAbsPaths[absP] = struct{}{}
				if rc.ShouldIgnore(absP) {
					return nil
				}
				return rc.stitchFile(w, path)
			},
		)
	}

	rc.seenAbsPaths[absP] = struct{}{}
	return rc.stitchFile(w, pathRoot)
}

const (
	fileSeparatorLines string = "========================================================"
	codeMarkerDepth4   string = "````"
)

func (rc *RunCtx) stitchFile(w io.Writer, path string) error {
	// Header format can be tuned for LLM prompts.
	if _, err := fmt.Fprintf(
		w,
		"\n%v\nFILE: %s\n%v\n",
		fileSeparatorLines,
		path,
		fileSeparatorLines,
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
	path = filepath.Clean(path)

	root = filepath.ToSlash(root)
	path = filepath.ToSlash(path)

	fmt.Printf("DEBUG root: %v\n", root)
	fmt.Printf("DEBUG path: %v\n", path)

	if root == path {
		return 0
	}

	rootCount := strings.Count(root, "/")
	pathCount := strings.Count(path, "/")
	return pathCount - rootCount
}
