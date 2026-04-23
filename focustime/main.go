package main

import (
	"context"
	"fmt"
	"log"
	"os"

	"github.com/Unique-Divine/jiyuu/focustime/src"
)

func main() {
	cfg, err := src.ResolveStartCfg(src.StartCfg{})
	if err != nil {
		log.Fatalf("focustime: %v", err)
	}
	appCmd := src.NewAppCmd(cfg)
	if err := appCmd.Run(context.Background(), os.Args); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}
