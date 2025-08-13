package cmds

import (
	"log"
	"os/exec"
	"strings"

	"github.com/urfave/cli/v2"
)

// Application binary name. THe name of the root command.
const CMD string = "ud"

// SHELL_BINARY_PATH is the local path to a Rust executable for a customizable
// bash that returns stdout and stdin as bytes.
var SHELL_BINARY_PATH string = "/home/realu/ki/jiyuu/mycli/run-bash-bin"

// ExecCommand: passes a string to the terminal as if it were run in the default
// shell, returning stdin and stdout as bytes, and an error if the execution fails.
func ExecCommand(in string) (bz []byte, err error) {
	binaryPath := SHELL_BINARY_PATH
	cmd := exec.Command(binaryPath, strings.Split(in, " ")...)
	out, err := cmd.Output()
	return out, err
}

// MustExecCommands: Runs multiple calls of 'ExecCommand', executing the input
// strings in the order given.
func MustExecCommands(ins []string) string {
	var outStrs []string
	for _, in := range ins {
		out, err := ExecCommand(in)
		outStrs = append(outStrs, string(out))
		if err != nil {
			log.Print(err)
		}
	}
	out := strings.Join(outStrs, "")
	return out
}

func AppendCliArgs(in string, ctx *cli.Context) (newIn string) {
	newIn = in
	if ctx.NArg() > 0 {
		var args []string = ctx.Args().Slice()
		newIn += " " + strings.Join(args, " ")
	}
	return newIn
}
