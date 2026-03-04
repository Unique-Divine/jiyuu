package main

import (
	"context"
	"fmt"
	"log"
	"os"

	"github.com/Unique-Divine/jiyuu/focustime/src"
)

func main() {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		log.Fatal(err)
	}
	cfg := src.StartCfg{HomeDir: homeDir}
	appCmd := src.NewAppCmd(cfg)
	if err := appCmd.Run(context.Background(), os.Args); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}
