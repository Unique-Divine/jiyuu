package src

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/stretchr/testify/require"
	cli "github.com/urfave/cli/v3"
)

func setupCLIApp(t *testing.T) (StartCfg, *bytes.Buffer, *bytes.Buffer, *cli.Command) {
	t.Helper()
	newHome := t.TempDir()
	t.Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	require.NoError(t, SaveAreasFile(cfg, reg))

	stdout := new(bytes.Buffer)
	stderr := new(bytes.Buffer)
	app := NewAppCmd(cfg)
	attachWriters(app, stdout, stderr)
	return cfg, stdout, stderr, app
}

func attachWriters(cmd *cli.Command, out *bytes.Buffer, err *bytes.Buffer) {
	cmd.Writer = out
	cmd.ErrWriter = err
	for _, sub := range cmd.Commands {
		attachWriters(sub, out, err)
	}
}

func TestCLILogActionSuccess(t *testing.T) {
	cfg, stdout, _, app := setupCLIApp(t)
	err := app.Run(context.Background(), []string{"focustime", "log", "45m", "1"})
	require.NoError(t, err)
	require.Contains(t, stdout.String(), "Logged 45m to area 1")

	now := time.Now().UTC()
	woy := TimeToWoY(now)
	yf, err := LoadYearFile(cfg, woy.Year)
	require.NoError(t, err)
	week := yf.Weeks[woy.WeekIndex()]
	require.NotNil(t, week)
	row := findAreaRowIndex(week, 1)
	require.NotEqual(t, -1, row)
	day := WeekdayToDayIndex(now.Weekday())
	require.NotNil(t, week.Values[row][day])
}

func TestCLILogActionMissingArgs(t *testing.T) {
	_, _, stderr, app := setupCLIApp(t)
	err := app.Run(context.Background(), []string{"focustime", "log"})
	require.NoError(t, err)
	require.Contains(t, stderr.String(), "focustime log: requires <time_amt> <area_idx>")
}

func TestCLILogActionInvalidAreaArg(t *testing.T) {
	_, _, _, app := setupCLIApp(t)
	err := app.Run(context.Background(), []string{"focustime", "log", "45m", "abc"})
	require.Error(t, err)
	require.Contains(t, err.Error(), "invalid area id")
}

func TestCLITodayActionWritesReport(t *testing.T) {
	_, stdout, _, app := setupCLIApp(t)
	err := app.Run(context.Background(), []string{"focustime", "log", "30m", "0"})
	require.NoError(t, err)
	stdout.Reset()

	err = app.Run(context.Background(), []string{"focustime", "today"})
	require.NoError(t, err)
	require.Contains(t, stdout.String(), "Today (")
	require.Contains(t, stdout.String(), "0 → A: 30m")
	require.Contains(t, stdout.String(), "Total: 30m")
}

func TestCLIWeekActionWritesReport(t *testing.T) {
	_, stdout, _, app := setupCLIApp(t)
	err := app.Run(context.Background(), []string{"focustime", "log", "15m", "2"})
	require.NoError(t, err)
	stdout.Reset()

	err = app.Run(context.Background(), []string{"focustime", "week"})
	require.NoError(t, err)
	require.Contains(t, stdout.String(), "# focustime week view")
	require.Contains(t, stdout.String(), "# Area")
	require.Contains(t, stdout.String(), "|   日 |   月 |   火 |   水 |   木 |   金 |   土")
	require.Contains(t, stdout.String(), "C     |")
}

// TestCLIAreasEditActionUpdatesAreas verifies end-to-end CLI behavior for
// `areas edit`: the command launches the editor path, parses loose input, trims
// values, and persists the resulting ordered areas list.
func TestCLIAreasEditActionUpdatesAreas(t *testing.T) {
	cfg, stdout, _, app := setupCLIApp(t)
	editorScript := filepath.Join(t.TempDir(), "fake-editor.sh")
	script := strings.Join([]string{
		"#!/usr/bin/env bash",
		"cat <<'EOF' > \"$1\"",
		"# focustime areas edit",
		"[",
		"Deep Work,",
		" Code Review  ,",
		"# notes",
		"Exercise",
		"]",
		"EOF",
		"",
	}, "\n")
	require.NoError(t, os.WriteFile(editorScript, []byte(script), 0o755))
	t.Setenv("EDITOR", editorScript)

	err := app.Run(context.Background(), []string{"focustime", "areas", "edit"})
	require.NoError(t, err)
	require.Contains(t, stdout.String(), "Updated 3 areas")

	reg, err := LoadAreasFile(cfg)
	require.NoError(t, err)
	require.Equal(t, []string{"Deep Work", "Code Review", "Exercise"}, reg.Areas)
}

func TestCLIAreasSaveLayoutActionSavesAndSetsLastUsed(t *testing.T) {
	cfg, stdout, _, app := setupCLIApp(t)
	err := app.Run(context.Background(), []string{"focustime", "areas", "save-layout"})
	require.NoError(t, err)
	require.Contains(t, stdout.String(), "Saved layout")
	require.Contains(t, stdout.String(), "with 3 areas")

	reg, err := LoadAreasFile(cfg)
	require.NoError(t, err)
	require.Len(t, reg.AreaLayouts, 2)
	require.Equal(t, []int{0, 1, 2}, reg.AreaLayouts[1])
	require.Equal(t, uint8(1), reg.LastUsedAreaLayoutIndex)
}
