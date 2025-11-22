package main

import (
	"fmt"
	"os"

	aictx "github.com/Unique-Divine/jiyuu/aictx/src"
)

func main() {
	cliApp := aictx.NewCliApp()
	if err := cliApp.Run(os.Args); err != nil {
		fmt.Fprintln(os.Stderr, "error: ", err)
		os.Exit(1)
	}
}
