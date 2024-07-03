package cmds

import (
	"fmt"

	"github.com/urfave/cli/v2"
)

var GoCmdStrings = map[string]string{
	"test-short":  "go test ./... -short -cover 2>&1",
	"test-int":    "go test ./... -run Integration -cover 2>&1",
	"test":        "go test ./... -cover 2>&1",
	"lint":        "golangci-lint run --allow-parallel-runners --fix",
	"cover-short": `go test ./... -short -cover  -coverprofile="temp.out" 2>&1`,
	"cover":       `go test ./... -cover  -coverprofile="temp.out" 2>&1`,
}

func newShowCmdFlag() *cli.BoolFlag {
	return &cli.BoolFlag{
		Name:  "cmd",
		Usage: "Displays the underlying command",
	}
}

func runShowCmdFlag(
	ctx *cli.Context, show string,
) (stop bool) {
	isShowCmd := ctx.Bool("cmd")
	if isShowCmd {
		fmt.Println(show)
	}
	return isShowCmd
}

func CmdGo() *cli.Command {

	return &cli.Command{
		Name:        "go",
		Aliases:     []string{},
		Usage:       "Golang-specific commands",
		Description: "",
		Action: func(*cli.Context) error {
			out, err := ExecCommand(fmt.Sprintf("%s go help", CMD))
			fmt.Printf("%s", out)
			return err
		},
		Subcommands: []*cli.Command{
			{
				Name:    "test-short",
				Aliases: []string{"ts"},
				Usage:   "Run Golang unit tests with coverage report",
				Action: func(ctx *cli.Context) (err error) {
					in := GoCmdStrings["test-short"]
					in = AppendCliArgs(in, ctx)
					in = in + ` | grep -v "no test" | grep -v "no statement"`

					if runShowCmdFlag(ctx, in) {
						return
					}

					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
				Flags: []cli.Flag{
					newShowCmdFlag(),
				},
			},

			{
				Name:    "test-int",
				Aliases: []string{"ti"},
				Usage:   "Run Golang integration tests with coverage report",
				Action: func(ctx *cli.Context) (err error) {
					in := GoCmdStrings["test-int"]
					in = AppendCliArgs(in, ctx)
					in = in + ` | grep -v "no test" | grep -v "no statement"`
					if runShowCmdFlag(ctx, in) {
						return
					}
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
				Flags: []cli.Flag{
					newShowCmdFlag(),
				},
			},

			{
				Name:    "lint",
				Aliases: []string{},
				Usage:   "Run golangci-lint",
				Action: func(ctx *cli.Context) (err error) {
					in := GoCmdStrings["lint"]
					in = AppendCliArgs(in, ctx)
					if runShowCmdFlag(ctx, in) {
						return
					}
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
				Flags: []cli.Flag{
					newShowCmdFlag(),
				},
			},

			{
				Name:    "test",
				Aliases: []string{"t"},
				Usage:   "Run Golang tests",
				Action: func(ctx *cli.Context) (err error) {
					in := GoCmdStrings["test"]
					in = AppendCliArgs(in, ctx)
					in = in + ` | grep -v "no test" | grep -v "no statement"`
					if runShowCmdFlag(ctx, in) {
						return
					}
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
				Flags: []cli.Flag{
					newShowCmdFlag(),
				},
			},

			{
				Name:    "cover-short",
				Aliases: []string{"cs"},
				Usage:   "Run Go short tests and coverage html",
				Action: func(ctx *cli.Context) error {
					in := GoCmdStrings["cover-short"]
					in = AppendCliArgs(in, ctx)
					in = in + ` | grep -v "no test" | grep -v "no statement"`
					MustExecCommands([]string{
						in,
						`go tool cover -html="temp.out" -o coverage.html`,
						`explorer.exe coverage.html`,
					})
					return nil
				},
				Flags: []cli.Flag{
					newShowCmdFlag(),
				},
			},

			{
				Name:    "cover",
				Aliases: []string{"c"},
				Usage:   "Run Go tests and coverage html",
				Action: func(ctx *cli.Context) error {
					in := GoCmdStrings["cover"]
					in = AppendCliArgs(in, ctx)
					in = in + ` | grep -v "no test" | grep -v "no statement"`
					MustExecCommands([]string{
						in,
						`go tool cover -html="temp.out" -o coverage.html`,
						`explorer.exe coverage.html`,
					})
					return nil
				},
				Flags: []cli.Flag{
					newShowCmdFlag(),
				},
			},
		},
	}
}
