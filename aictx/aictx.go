package main

import (
	"bytes"
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
		Usage:     "Combine files into a single LLM-friendly output and copy it to the clipboard",
		ArgsUsage: "<path> [path2 ...]",
		Flags: []cli.Flag{
			&cli.StringFlag{
				Name:    "output",
				Aliases: []string{"o"},
				Usage:   "write result to file in addition to copying it to the clipboard",
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
	outputPath := c.String("output")
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

	argPaths, err := ExpandGlobs(rawCmdArgs)
	if err != nil {
		return err
	}
	if len(argPaths) == 0 {
		return cli.Exit("no paths matched", 1)
	}

	var buf bytes.Buffer
	if err := stitchPaths(&buf, argPaths); err != nil {
		return err
	}
	data := buf.Bytes()

	if outputPath != "" {
		if err := os.WriteFile(outputPath, data, 0o644); err != nil {
			return err
		}
	}

	if err := copyToClipboard(data); err != nil {
		return err
	}

	// Print to stdout so you can see the stitched result in the terminal.
	_, err = os.Stdout.Write(data)
	return err
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

func copyToClipboard(data []byte) error {
	cmd := exec.Command("pbcopy")
	cmd.Stdin = bytes.NewReader(data)
	return cmd.Run()
}

func stitchPaths(w io.Writer, paths []string) error {
	rc := NewRunCtx()
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
				return rc.stitchFile(w, path)
			},
		)
	}
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

type RunCtx struct {
	Ignorer      gitignore.GitIgnore
	ignoreLines  []string
	ignoreFiles  map[string]struct{}
	seenAbsPaths map[string]struct{}
}

func NewRunCtx() *RunCtx {
	return &RunCtx{
		Ignorer:      gitignore.GitIgnore{},
		seenAbsPaths: make(map[string]struct{}),
		ignoreFiles:  make(map[string]struct{}),
	}
}
