package cmds_test

import (
	"fmt"
	"math/rand"
	"os"
	"testing"

	"ud/cmds"

	"github.com/stretchr/testify/require"
)

func TestBinaryExists(t *testing.T) {
	_, err := os.Stat(cmds.SHELL_BINARY_PATH)
	require.NoError(t, err)
}

func TestExecCmd(t *testing.T) {
	in := "ls"
	bz, err := cmds.ExecCommand(in)
	require.NoErrorf(t, err, string(bz))

	in = "pwd"
	bz, err = cmds.ExecCommand(in)
	require.NoErrorf(t, err, string(bz))

	tempDirId := rand.Int63n(420)
	ins := []string{
		fmt.Sprintf("mkdir tmp%v", tempDirId),
		fmt.Sprintf("touch tmp%v/foo1", tempDirId),
		fmt.Sprintf("touch tmp%v/foo2", tempDirId),
		fmt.Sprintf("rmdir tmp%v", tempDirId),
		fmt.Sprintf("rm -r tmp%v", tempDirId),
	}

	require.NotPanics(t, func() {
		cmds.MustExecCommands(ins)
	})
}
