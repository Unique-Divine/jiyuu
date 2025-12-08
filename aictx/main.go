package main

import (
	"context"
	"fmt"
	"os"

	aictx "github.com/Unique-Divine/jiyuu/aictx/src"
)

func main() {
	appCmd := aictx.NewAppCmd()
	if err := appCmd.Run(context.Background(), os.Args); err != nil {
		fmt.Fprintln(appCmd.ErrWriter, "error: ", err)
		os.Exit(1)
	}
}
