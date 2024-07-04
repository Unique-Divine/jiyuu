package cmds

import (
	"fmt"

	"github.com/urfave/cli/v2"
)

// CmdQuick: Implements the help command connectd to the zsh/quick.sh script from
// my dotfiles.
func CmdQuick() *cli.Command {
	return &cli.Command{
		Name:    "quick",
		Aliases: []string{"q", "cfg"},
		Usage:   "Core configuration commands and common jumps to editors",
		Description: `Quick jumps to open Neovim (nvim) to do different working directories..
In the below commands, "edit" means "open nvim with a certain working directory".

cfg_nvim     Edit nvim (Neovim) config
cfg_tmux     Edit tmux config
myrc         Edit your zshrc config
music        Opens the Windows file explorer to your music files
dotf         Edit your dotfiles
notes        Edit your notes workspace
todos        Edit your notes workspace with your text-based TODO-list open
`,
		Action: func(*cli.Context) error {
			in := fmt.Sprintf("%s quick help", CMD)
			out, err := ExecCommand(in)
			fmt.Printf("%s", out)
			return err
		},
		// NOTE: Subcommands are unused here because this is a pure help command.
		Subcommands: []*cli.Command{
			// {
			// 	Name:    "nvim",
			// 	Aliases: []string{"vim", "vi"},
			// 	Usage:   "Edit nvim (NeoVim) config",
			// 	Action: func(ctx *cli.Context) error {
			// 		// Retrieve crate name from Cargo.tol
			// 		fmt.Println("cfg_nvim")
			// 		return nil
			// 	},
			// },

		},
	}
}
