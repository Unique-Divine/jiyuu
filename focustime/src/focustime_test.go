package src

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/stretchr/testify/suite"
)

func TestSrc(t *testing.T) {
	suite.Run(t, new(S))
}

type S struct {
	suite.Suite
}

func (s *S) TestTimeToWeek() {
	type TC struct {
		t    time.Time
		want WoY
	}
	for tcIdx, tc := range []TC{
		{
			// Week 1 of 2025
			t:    time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC),
			want: WoY{Year: 2025, Week: 1},
		},
		{
			// Week 50 of 2025
			t:    time.Date(2025, 12, 8, 0, 0, 0, 0, time.UTC),
			want: WoY{Year: 2025, Week: 50},
		},
	} {
		s.Run(fmt.Sprintf("tc %d, %#v", tcIdx, tc), func() {
			got := TimeToWoY(tc.t)
			s.Equal(tc.want, got)
		})
	}
	// Week 1 of 2024
}

func (s *S) TestCreateFocusTimeDir() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{
		HomeDir: newHome,
	}

	ftDir := DirFTData(cfg)
	_, err := os.Stat(ftDir)
	s.True(os.IsNotExist(err), "expect focustime data dir to not exist yet")

	err = CreateFocusTimeDir(cfg)
	s.NoError(err)

	info, err := os.Stat(ftDir)
	s.NoError(err)
	s.True(info.IsDir(), "focustime dir should now exist.")
}

func (s *S) TestLoadAreasFile() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	areasPath := filepath.Join(DirFTData(cfg), "areas.json")
	_, err := os.Stat(areasPath)
	s.True(os.IsNotExist(err), "areas.json should not exist yet")

	got, err := LoadAreasFile(cfg)
	s.NoError(err)
	s.Empty(got.Areas)
	s.Empty(got.AreaLayouts)
	s.Equal(uint8(0), got.LastUsedAreaLayoutIndex)

	info, err := os.Stat(areasPath)
	s.NoError(err)
	s.False(info.IsDir(), "areas.json should be a file, not a directory")
}
