package src

import (
	"context"
	"fmt"
	"strconv"
	"time"

	cli "github.com/urfave/cli/v3"
)

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
				Name:  "areas",
				Usage: "Manage focus areas",
				Commands: []*cli.Command{
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
				Action: areasListAction(cfg),
			},
		},
		Action: areasListAction(cfg),
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
