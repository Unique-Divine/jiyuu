package cmds

import (
	"fmt"

	"github.com/urfave/cli/v2"
)

// CmdMarkdown: Markdown subcommand
func CmdMarkdown() *cli.Command {

	return &cli.Command{
		Name:        "md",
		Aliases:     []string{},
		Usage:       "Markdown commands",
		Description: "For any markdown utilities",
		Action: func(*cli.Context) error {
			in := fmt.Sprintf("%s md help", CMD)
			out, err := ExecCommand(in)
			fmt.Printf("%s", out)
			return err
		},
		Subcommands: []*cli.Command{
			{
				Name:  "show",
				Usage: "Show markdown preview in browser",
				Action: func(ctx *cli.Context) error {
					in := "markdown-preview"
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					if err != nil {
						return fmt.Errorf(`%w: install with "bun install -g @mryhryki/markdown-preview"`, err)
					}
					return err
				},
			},
		},
	}
}
