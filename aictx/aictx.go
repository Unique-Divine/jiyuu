package main

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"

	cli "github.com/urfave/cli/v2"
)

func main() {
	app := &cli.App{
		Name:      "aictx",
		Usage:     "Combine files into a single LLM-friendly output and copy it to the clipboard",
		ArgsUsage: "<path> [path2 ...]",
		Flags: []cli.Flag{
			&cli.StringFlag{
				Name:    "output",
				Aliases: []string{"o"},
				Usage:   "write result to file in addition to copying it to the clipboard",
			},
		},
		Action: runCat,
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
				Action: runCat,
			},
		},
	}

	if err := app.Run(os.Args); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

func runCat(c *cli.Context) error {
	outputPath := c.String("output")
	rawArgs := c.Args().Slice()

	if len(rawArgs) == 0 {
		return cli.Exit("cat requires at least one path (file, directory, or glob)", 1)
	}

	paths, err := expandGlobs(rawArgs)
	if err != nil {
		return err
	}
	if len(paths) == 0 {
		return cli.Exit("no paths matched", 1)
	}

	var buf bytes.Buffer
	if err := stitchFiles(&buf, paths); err != nil {
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

	// Optional: also print to stdout so you can see the stitched result in the terminal.
	if _, err := os.Stdout.Write(data); err != nil {
		return err
	}

	return nil
}

func expandGlobs(args []string) ([]string, error) {
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

func stitchFiles(w io.Writer, paths []string) error {
	for _, p := range paths {
		if err := stitchPath(w, p); err != nil {
			return err
		}
	}
	return nil
}

func stitchPath(w io.Writer, path string) error {
	info, err := os.Stat(path)
	if err != nil {
		return err
	}

	if info.IsDir() {
		return stitchDir(w, path)
	}
	return stitchFile(w, path)
}

const fileSeparatorLines string = "========================================================"
const codeMarkerDepth4 string = "````"

func stitchFile(w io.Writer, path string) error {
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

func stitchDir(w io.Writer, root string) error {
	return filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		return stitchFile(w, path)
	})
}
