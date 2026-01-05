package src

import (
	"bytes"
	"context"
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
	require.Contains(t, stdout.String(), "# Columns: Area | Mon | Tue | Wed | Thu | Fri | Sat | Sun")
	require.Contains(t, stdout.String(), "C |")
}
