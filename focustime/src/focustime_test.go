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

func (s *S) TestSaveAreasFile() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg := FocusAreas{
		Areas:                   []string{"Deep Work", "Coding", "Exercise"},
		AreaLayouts:             [][]int{{0, 1, 2}},
		LastUsedAreaLayoutIndex: 0,
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	areasPath := filepath.Join(DirFTData(cfg), "areas.json")
	info, err := os.Stat(areasPath)
	s.NoError(err)
	s.False(info.IsDir(), "areas.json should be a file")

	got, err := LoadAreasFile(cfg)
	s.NoError(err)
	s.Equal(reg.Areas, got.Areas)
	s.Equal(reg.AreaLayouts, got.AreaLayouts)
	s.Equal(reg.LastUsedAreaLayoutIndex, got.LastUsedAreaLayoutIndex)
}

func (s *S) TestAddArea() {
	reg := FocusAreas{Areas: []string{}, AreaLayouts: [][]int{}, LastUsedAreaLayoutIndex: 0}
	got := AddArea(reg, "Deep Work")
	s.Equal([]string{"Deep Work"}, got.Areas)

	reg2 := FocusAreas{
		Areas:       []string{"A", "B"},
		AreaLayouts: [][]int{},
	}
	got2 := AddArea(reg2, "C")
	s.Equal([]string{"A", "B", "C"}, got2.Areas)
}

func (s *S) TestRemoveArea() {
	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	got, err := RemoveArea(reg, 0)
	s.NoError(err)
	s.Equal([]string{"B", "C"}, got.Areas)
	s.Equal([][]int{{0, 1}}, got.AreaLayouts)

	got2, err := RemoveArea(reg, 1)
	s.NoError(err)
	s.Equal([]string{"A", "C"}, got2.Areas)
	s.Equal([][]int{{0, 1}}, got2.AreaLayouts)

	_, err = RemoveArea(reg, -1)
	s.Error(err)
	_, err = RemoveArea(reg, 3)
	s.Error(err)
}

func (s *S) TestRemoveAreaUpdatesLayouts() {
	reg := FocusAreas{
		Areas:                   []string{"A", "B", "C"},
		AreaLayouts:             [][]int{{0, 1, 2}},
		LastUsedAreaLayoutIndex: 0,
	}
	got, err := RemoveArea(reg, 1)
	s.NoError(err)
	s.Equal([]string{"A", "C"}, got.Areas)
	s.Equal([][]int{{0, 1}}, got.AreaLayouts) // old 0,2 became 0,1
}

func (s *S) TestLoadSaveRoundtrip() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg, err := LoadAreasFile(cfg)
	s.NoError(err)
	s.Empty(reg.Areas)

	reg = AddArea(reg, "Deep Work")
	reg = AddArea(reg, "Coding")
	err = SaveAreasFile(cfg, reg)
	s.NoError(err)

	got, err := LoadAreasFile(cfg)
	s.NoError(err)
	s.Equal([]string{"Deep Work", "Coding"}, got.Areas)
}
