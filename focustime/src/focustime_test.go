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

func (s *S) TestGetDefaultAreaLayout() {
	cfg := StartCfg{HomeDir: s.T().TempDir()}

	// last_used valid
	reg := FocusAreas{
		Areas:                   []string{"A", "B", "C"},
		AreaLayouts:             [][]int{{0, 1}, {0, 1, 2}},
		LastUsedAreaLayoutIndex: 1,
	}
	yf := emptyYearFile(2025)
	got, err := GetDefaultAreaLayout(cfg, reg, &yf)
	s.NoError(err)
	s.Equal([]int{0, 1, 2}, got)

	// last_used invalid (empty layouts) → most recent week
	reg2 := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{},
	}
	yf2 := emptyYearFile(2025)
	yf2.Weeks[9] = &WeekValues{Areas: []int{0, 1, 2}, Values: [][]*int{{nil}, {nil}, {nil}}}
	got2, err := GetDefaultAreaLayout(cfg, reg2, &yf2)
	s.NoError(err)
	s.Equal([]int{0, 1, 2}, got2)

	// no layouts, no weeks → bootstrap
	reg3 := FocusAreas{
		Areas:       []string{"X", "Y", "Z"},
		AreaLayouts: [][]int{},
	}
	yf3 := emptyYearFile(2025)
	got3, err := GetDefaultAreaLayout(cfg, reg3, &yf3)
	s.NoError(err)
	s.Equal([]int{0, 1, 2}, got3)

	// bootstrap with 2 areas → error
	reg4 := FocusAreas{
		Areas:       []string{"A", "B"},
		AreaLayouts: [][]int{},
	}
	yf4 := emptyYearFile(2025)
	_, err = GetDefaultAreaLayout(cfg, reg4, &yf4)
	s.Error(err)
}

func (s *S) TestRenderWeekBuffer() {
	v120, v90, v60 := 120, 90, 60
	week := WeekValues{
		Areas: []int{0, 1, 2},
		Values: [][]*int{
			{&v120, &v90, &v60, nil, nil, nil, nil},
			{nil, nil, nil, nil, nil, nil, nil},
			{&v60, nil, nil, nil, nil, nil, &v60},
		},
	}
	areaNames := []string{"Deep Work", "Coding", "Exercise"}
	weekStart := time.Date(2025, 3, 3, 0, 0, 0, 0, time.UTC)
	got := RenderWeekBuffer(week, areaNames, 2025, 9, weekStart)
	s.Contains(got, "# focustime week view")
	s.Contains(got, "# Week index: 2025w10 (Week starting: 2025-03-03)")
	s.Contains(got, "# Columns: Area | Mon | Tue | Wed | Thu | Fri | Sat | Sun")
	s.Contains(got, "Deep Work |")
	s.Contains(got, "120")
	s.Contains(got, "90")
	s.Contains(got, "Exercise")
}

func (s *S) TestParseWeekBuffer() {
	buf := `# focustime week view
# Week index: 2025w10 (Week starting: 2025-03-03)
# Columns: Area | Mon | Tue | Wed | Thu | Fri | Sat | Sun

Deep Work | 120 |  90 |  60 |   0 |   0 |     |
Coding    |  30 |  30 |  45 |     |   0 |     |
Exercise  |  60 |   0 |     |  30 |   0 |  0 |  60
`
	areaNames := []string{"Deep Work", "Coding", "Exercise"}
	areaIDs := []int{0, 1, 2}
	got, err := ParseWeekBuffer([]byte(buf), areaNames, areaIDs)
	s.NoError(err)
	s.Equal([]int{0, 1, 2}, got.Areas)
	s.Len(got.Values, 3)
	// row 0
	s.Equal(120, *got.Values[0][0])
	s.Equal(90, *got.Values[0][1])
	s.Equal(60, *got.Values[0][2])
	s.Equal(0, *got.Values[0][3])
	s.Equal(0, *got.Values[0][4])
	s.Nil(got.Values[0][5])
	s.Nil(got.Values[0][6])
	// row 1 - blank at index 3
	s.Equal(30, *got.Values[1][0])
	s.Nil(got.Values[1][3])
	// row 2
	s.Equal(60, *got.Values[2][6])

	// invalid int
	badBuf := `# header
Area | 120 | x | 60 | 0 | 0 | 0 | 0
`
	_, err = ParseWeekBuffer([]byte(badBuf), []string{"Area"}, []int{0})
	s.Error(err)
}

func (s *S) TestRenderParseRoundtrip() {
	v120, v90, v0 := 120, 90, 0
	week := WeekValues{
		Areas: []int{0, 1, 2},
		Values: [][]*int{
			{&v120, &v90, nil, &v0, nil, nil, nil},
			{&v0, nil, nil, nil, nil, nil, nil},
			{nil, nil, nil, nil, nil, nil, &v90},
		},
	}
	areaNames := []string{"Deep Work", "Coding", "Exercise"}
	weekStart := time.Date(2025, 3, 3, 0, 0, 0, 0, time.UTC)
	buf := RenderWeekBuffer(week, areaNames, 2025, 9, weekStart)
	parsed, err := ParseWeekBuffer([]byte(buf), areaNames, week.Areas)
	s.NoError(err)
	s.Equal(week.Areas, parsed.Areas)
	s.Len(parsed.Values, len(week.Values))
	for row := range week.Values {
		for d := 0; d < 7; d++ {
			if week.Values[row][d] == nil {
				s.Nil(parsed.Values[row][d], "row %d day %d should be nil", row, d)
			} else {
				s.NotNil(parsed.Values[row][d])
				s.Equal(*week.Values[row][d], *parsed.Values[row][d])
			}
		}
	}
}

func (s *S) TestParseDurationToMinutes() {
	type TC struct {
		in      string
		wantMin int
		wantErr bool
	}
	for _, tc := range []TC{
		{in: "2h", wantMin: 120},
		{in: "45m", wantMin: 45},
		{in: "1.5h", wantMin: 90},
		{in: "30s", wantErr: true},
		{in: "bad", wantErr: true},
	} {
		got, err := ParseDurationToMinutes(tc.in)
		if tc.wantErr {
			s.Error(err)
			continue
		}
		s.NoError(err)
		s.Equal(tc.wantMin, got)
	}
}
