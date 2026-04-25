package main

import (
	"context"
	"fmt"
	"os"

	ctxcat "github.com/Unique-Divine/jiyuu/ctxcat/src"
)

func main() {
	appCmd := ctxcat.NewAppCmd()
	if err := appCmd.Run(context.Background(), os.Args); err != nil {
		fmt.Fprintln(appCmd.ErrWriter, "error: ", err)
		os.Exit(1)
	}
}
