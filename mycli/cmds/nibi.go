package cmds

import (
	"fmt"

	"github.com/urfave/cli/v2"
)

func CmdNibi() *cli.Command {
	return &cli.Command{
		Name:    "nibi",
		Aliases: []string{},
		Usage:   "Nibiru-specific commands",
		Action: func(*cli.Context) error {
			in := fmt.Sprintf("%s nibi help", CMD)
			out, err := ExecCommand(in)
			fmt.Printf("%s", out)
			return err
		},
		Subcommands: []*cli.Command{
			cmdNibiCfg(),
			{
				Name:    "addrs",
				Aliases: []string{},
				Usage:   "Show all comman Nibiru addresses used in testing",
				Action: func(*cli.Context) error {
					ins := []string{
						"echo $ADDR_VAL ADDR_VAL",
						"echo $ADDR_UD ADDR_UD",
						`echo $ADDR_DELPHI ADDR_DELPHI`,
						`echo $FAUCET_WEB FAUCET_WEB`,
						`echo $FAUCET_DISCORD FAUCET_DISCORD`,
					}
					out := MustExecCommands(ins)
					fmt.Print(string(out))
					return nil
				},
			},
			{
				Name:    "get-nibid",
				Aliases: []string{"gn"},
				Usage:   "Curl the nibid binary",
				Action: func(*cli.Context) error {
					ins := []string{
						`curl -s https://get.nibiru.fi/@v0.19.2! | bash`,
					}
					out := MustExecCommands(ins)
					fmt.Print(string(out))
					return nil
				},
			},
		},
	}
}

// cmdNibiCfg subcommand of [CmdNibi] that changes the Nibiru CLI config.
func cmdNibiCfg() *cli.Command {
	return &cli.Command{
		Name:    "cfg",
		Aliases: []string{},
		Usage:   "Set Nibiru CLI config to a network",
		Action: func(*cli.Context) error {
			in := fmt.Sprintf("%s nibi cfg help", CMD)
			out, err := ExecCommand(in)
			fmt.Printf("%s", out)
			return err
		},
		Subcommands: []*cli.Command{
			{
				Name:  "local",
				Usage: "Local network (localnet)",
				Action: func(*cli.Context) error {
					in := `cfg_nibi_local`
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
			},
			{
				Name:    "prod",
				Aliases: []string{"mainnet"},
				Usage:   "Mainnet (cataclysm-1)",
				Action: func(*cli.Context) error {
					in := `cfg_nibi`
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
			},
			{
				Name:  "test",
				Usage: "Test network (testnet)",
				Action: func(*cli.Context) error {
					in := `cfg_nibi_test`
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
			},
			{
				Name:  "dev",
				Usage: "Development network (devnet)",
				Action: func(*cli.Context) error {
					in := `cfg_nibi_dev`
					out, err := ExecCommand(in)
					fmt.Printf("%s", out)
					return err
				},
			},
		},
	}
}
