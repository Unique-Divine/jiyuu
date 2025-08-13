package cmds

import (
	"fmt"
	"strings"

	"github.com/urfave/cli/v2"
)

const (
	RUST_CMD_LINT       = "cargo clippy --fix --allow-dirty --allow-staged"
	RUST_CMD_LINT_CHECK = "cargo clippy"
	RUST_CMD_FMT        = "cargo fmt --all"
)

// CmdRust: Rust subcommand for common cargo, rustup, and rustc commands
func CmdRust() *cli.Command {
	return &cli.Command{
		Name:        "rs",
		Aliases:     []string{},
		Usage:       "Rust-specific commands",
		Description: "For common cargo, rustup, and rustc commands",
		Action: func(*cli.Context) error {
			in := fmt.Sprintf("%s rs help", CMD)
			out, err := ExecCommand(in)
			fmt.Printf("%s", out)
			return err
		},
		Subcommands: []*cli.Command{
			{
				Name:    "test-short",
				Aliases: []string{"ts"},
				Usage:   "Run run tests for the current package",
				Action: func(ctx *cli.Context) error {
					// Retrieve crate name from Cargo.toml
					in := `name=$(grep -e "^name = " Cargo.toml | cut -d= -f2- | tr -d '"[:space:]')`
					useCrateName := true
					out, err := ExecCommand(in)
					if err != nil {
						fmt.Println(err.Error())
						useCrateName = false
					}
					fmt.Printf("out: %v\n", out)

					out, _ = ExecCommand("echo $name")
					crateName := string(out)

					switch {
					case useCrateName:
						fmt.Printf("crateName: %v\n", crateName)
						cmd := fmt.Sprintf(`pkg=%v`, crateName)
						_, _ = ExecCommand(cmd)
						cmd = fmt.Sprintf(
							`RUST_BACKTRACE="1" cargo test --package "%v"`,
							crateName)
						out, err = ExecCommand(cmd)
					default:
						out, err = ExecCommand(`RUST_BACKTRACE="1" cargo test`)
					}
					fmt.Printf("%s", out)
					return err
				},
			},

			{
				Name:    "test",
				Usage:   "Rust tests",
				Aliases: []string{"t"},
				Action: func(ctx *cli.Context) (err error) {
					in := `cargo test`
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
				Name:  "fmt",
				Usage: "Format with rustfmt",
				Action: func(ctx *cli.Context) (err error) {
					in := []string{
						`cp $KOJIN_PATH/dotfiles/rustfmt.toml .`,
						`cargo fmt --all`,
					}
					out := MustExecCommands(in)
					fmt.Printf("%s", out)
					return err
				},
			},

			{
				Name:  "tidy",
				Usage: "Build, lint, format",
				Action: func(ctx *cli.Context) error {
					in := strings.Join([]string{
						"cargo b",
						CMD + " rs clippy",
						CMD + " rs fmt",
					}, " && ")
					in = AppendCliArgs(in, ctx)
					fmt.Println(in)
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
			},

			{
				Name:    "lint",
				Usage:   "Run linter + fixes",
				Aliases: []string{"clippy"},
				Action: func(ctx *cli.Context) error {
					in := RUST_CMD_LINT
					in = AppendCliArgs(in, ctx)
					fmt.Println(in)
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
			},

			{
				Name:  "clippy-check",
				Usage: "Run linter (check only)",
				Action: func(ctx *cli.Context) error {
					in := RUST_CMD_LINT_CHECK
					in = AppendCliArgs(in, ctx)
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
			},
		},
	}
}
