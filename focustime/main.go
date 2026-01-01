package main

import (
	"fmt"
	"log"
	"os"

	"github.com/Unique-Divine/jiyuu/focustime/src"
)

func main() {
	fmt.Println("focustime main.go")
	homeDir, err := os.UserHomeDir()
	if err != nil {
		log.Fatal(err)
	}
	cfg := src.StartCfg{
		HomeDir: homeDir,
	}

	err = src.CreateFocusTimeDir(cfg)
	if err != nil {
		log.Fatal(err)
	}

	// src.LoadAreasFile()
}
