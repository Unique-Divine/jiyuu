package main

import (
	"fmt"
	"log"
	"os"

	"ud/cmds"

	"github.com/urfave/cli/v2"
)

func main() {
	app := &cli.App{
		Name:  cmds.CMD,
		Usage: "CLI for convenient bash execution",
		Action: func(ctx *cli.Context) error {
			out, err := cmds.ExecCommand(fmt.Sprintf(`%s help`, cmds.CMD))
			fmt.Println(string(out))
			return err
		},
		Commands: []*cli.Command{
			cmds.CmdGo(),
			cmds.CmdQuick(),
			cmds.CmdRust(),
			cmds.CmdNibi(),
			cmds.CmdMarkdown(),
		},
	}

	if err := app.Run(os.Args); err != nil {
		log.Fatal(err)
	}
}
