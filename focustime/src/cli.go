package src

import (
	"context"
	"fmt"
	"strconv"
	"time"

	cli "github.com/urfave/cli/v3"
)

func rootHelpAction(ctx context.Context, c *cli.Command) error {
	return cli.ShowAppHelp(c)
}

func subcommandHelpAction(ctx context.Context, c *cli.Command) error {
	return cli.ShowSubcommandHelp(c)
}

// NewAppCmd returns the root focustime CLI with edit and areas subcommands.
func NewAppCmd(cfg StartCfg) *cli.Command {
	return &cli.Command{
		Name:  "focustime",
		Usage: "Track focus time by areas",
		Commands: []*cli.Command{
			{
				Name:      "edit",
				Usage:     "Edit focus time for a week (opens editor)",
				ArgsUsage: "[YYYYwWW]",
				Action:    editAction(cfg),
			},
			{
				Name:   "today",
				Usage:  "Show today's logged values by area",
				Action: todayAction(cfg),
			},
			{
				Name:   "week",
				Usage:  "Show current week breakdown",
				Action: weekAction(cfg),
			},
			{
				Name:      "log",
				Usage:     "Log time to an area for today",
				ArgsUsage: "<time_amt> <area_idx>",
				Action:    logAction(cfg),
			},
			{
				Name:  "areas",
				Usage: "Manage focus areas",
				Commands: []*cli.Command{
					{
						Name:   "save-layout",
						Usage:  "Save current areas order as default layout",
						Action: areasSaveLayoutAction(cfg),
					},
					{
						Name:   "edit",
						Usage:  "Edit all areas in your editor",
						Action: areasEditAction(cfg),
					},
					{
						Name:      "add",
						Usage:     "Add an area",
						ArgsUsage: "<name>",
						Action:    areasAddAction(cfg),
					},
					{
						Name:      "rm",
						Usage:     "Remove an area by 0-based ID",
						ArgsUsage: "<id>",
						Action:    areasRmAction(cfg),
					},
					{
						Name:   "list",
						Usage:  "List all areas",
						Action: areasListAction(cfg),
					},
				},
				Action: subcommandHelpAction,
			},
		},
		Action: rootHelpAction,
	}
}

func areasAddAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		name := c.Args().First()
		if name == "" {
			_, _ = c.ErrWriter.Write([]byte("focustime areas add: name required\n"))
			return cli.ShowAppHelp(c)
		}
		reg, err := LoadAreasFile(cfg)
		if err != nil {
			return err
		}
		reg = AddArea(reg, name)
		if err := SaveAreasFile(cfg, reg); err != nil {
			return err
		}
		n := len(reg.Areas) - 1
		fmt.Fprintf(c.Writer, "Added area %d: %q\n", n, name)
		return nil
	}
}

func areasRmAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		arg := c.Args().First()
		if arg == "" {
			_, _ = c.ErrWriter.Write([]byte("focustime areas rm: id required\n"))
			return cli.ShowAppHelp(c)
		}
		id, err := strconv.Atoi(arg)
		if err != nil {
			return fmt.Errorf("invalid area id %q: %w", arg, err)
		}
		reg, err := LoadAreasFile(cfg)
		if err != nil {
			return err
		}
		reg, err = RemoveArea(reg, id)
		if err != nil {
			return err
		}
		if err := SaveAreasFile(cfg, reg); err != nil {
			return err
		}
		fmt.Fprintf(c.Writer, "Removed area %d\n", id)
		return nil
	}
}

func areasListAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		reg, err := LoadAreasFile(cfg)
		if err != nil {
			return err
		}
		if len(reg.Areas) == 0 {
			fmt.Fprintln(c.Writer, "No areas defined. Add some: focustime areas add \"Deep Work\"")
			return nil
		}
		for i, name := range reg.Areas {
			fmt.Fprintf(c.Writer, "%d → %s\n", i, name)
		}
		return nil
	}
}

func areasEditAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		if err := AreasEdit(cfg); err != nil {
			return err
		}
		reg, err := LoadAreasFile(cfg)
		if err != nil {
			return err
		}
		fmt.Fprintf(c.Writer, "Updated %d areas\n", len(reg.Areas))
		return nil
	}
}

func areasSaveLayoutAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		reg, err := LoadAreasFile(cfg)
		if err != nil {
			return err
		}
		reg, idx, err := SaveCurrentAreasAsLayout(reg)
		if err != nil {
			return err
		}
		if err := SaveAreasFile(cfg, reg); err != nil {
			return err
		}
		fmt.Fprintf(c.Writer, "Saved layout %d with %d areas and set as last used\n",
			idx, len(reg.Areas))
		return nil
	}
}

// editAction runs WeekEdit; arg is optional YYYYwWW (e.g. 2026w1).
func editAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		arg := c.Args().First()
		var woy WoY
		if arg == "" {
			woy = TimeToWoY(time.Now().UTC())
		} else {
			year, week, err := ParseYearWWeek(arg)
			if err != nil {
				return err
			}
			woy = WoY{Year: year, Week: week}
		}
		return WeekEdit(cfg, woy)
	}
}

func todayAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		out, err := TodayReport(cfg, time.Now().UTC())
		if err != nil {
			return err
		}
		fmt.Fprint(c.Writer, out)
		return nil
	}
}

func weekAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		out, err := CurrentWeekReport(cfg, time.Now().UTC())
		if err != nil {
			return err
		}
		fmt.Fprint(c.Writer, out)
		return nil
	}
}

func logAction(cfg StartCfg) cli.ActionFunc {
	return func(ctx context.Context, c *cli.Command) error {
		if c.Args().Len() < 2 {
			_, _ = c.ErrWriter.Write(
				[]byte("focustime log: requires <time_amt> <area_idx>\n"),
			)
			return cli.ShowAppHelp(c)
		}
		timeAmt := c.Args().Get(0)
		areaArg := c.Args().Get(1)
		areaID, err := strconv.Atoi(areaArg)
		if err != nil {
			return fmt.Errorf("invalid area id %q: %w", areaArg, err)
		}
		minutes, err := LogTime(cfg, timeAmt, areaID)
		if err != nil {
			return err
		}
		fmt.Fprintf(c.Writer, "Logged %dm to area %d\n", minutes, areaID)
		return nil
	}
}
