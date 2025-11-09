package main

import (
	"fmt"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/stretchr/testify/require"
)

// getScriptPath returns the absolute path to ud.sh
func getScriptPath() (string, error) {
	rootPath, err := FindRootPath()
	if err != nil {
		return "", err
	}
	return filepath.Join(rootPath, "ud.sh"), nil
}

// FindRootPath returns the absolute path of the repository root
// This is retrievable with: go list -m -f {{.Dir}}
func FindRootPath() (string, error) {
	// rootPath, _ := exec.Command("go list -m -f {{.Dir}}").Output()
	// This returns the path to the root of the project.
	rootPathBz, err := exec.Command("go", "list", "-m", "-f", "{{.Dir}}").Output()
	if err != nil {
		return "", err
	}
	rootPath := strings.Trim(string(rootPathBz), "\n")
	return rootPath, nil
}

// runUdCommand executes the ud command via bash and returns the output
func runUdCommand(t *testing.T, bashCmd string) string {
	scriptPath, err := getScriptPath()
	if err != nil {
		t.Fatalf("Failed to get script path: %v", err)
	}

	// Create a bash command that sources the script and runs the ud function
	// This mimics the "source ./ud.sh" behavior from your bash test
	cmdStr := fmt.Sprintf(
		"source %v && %v", scriptPath, bashCmd,
	)

	cmd := exec.Command("bash", "-c", cmdStr)
	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("Command failed: %v\nOutput: %s", err, output)
	}

	return strings.TrimSpace(string(output))
}

// pass is a helper that logs success (similar to your bash pass function)
func pass(t *testing.T, msg string) {
	t.Logf("✅ %s", msg)
}

// fail is a helper that fails the test (similar to your bash fail function)
func fail(t *testing.T, msg string) {
	t.Fatalf("😞❌ %s", msg)
}

func TestHelpCommand(t *testing.T) {
	for _, bashCmd := range []string{
		"ud",
		"ud help",
		"ud -h",
		"ud --help",
	} {
		output := runUdCommand(t, bashCmd)
		if !strings.Contains(output, "USAGE:") {
			fail(t, bashCmd)
		}
		pass(t, bashCmd)
	}
}

func TestRsTestCmd(t *testing.T) {
	bashCmd := "ud rs test --cmd"
	output := runUdCommand(t, bashCmd)
	if output != "cargo test" {
		fail(t, bashCmd)
	}
	pass(t, bashCmd)
}

func TestGoTestShortCmd(t *testing.T) {
	bashCmd := "ud go test-short --cmd"
	output := runUdCommand(t, bashCmd)
	if !strings.Contains(output, "go test ./...") {
		fail(t, bashCmd)
	}
	pass(t, bashCmd)
}

func TestNibiCfgProd(t *testing.T) {
	bashCmd := "ud nibi cfg prod"
	output := runUdCommand(t, bashCmd)
	require.Contains(t, output, "ud nibi cfg")
}
