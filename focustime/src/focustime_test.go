package src

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/mattn/go-runewidth"
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
	s.Contains(err.Error(), "focustime areas save-layout")
}

func (s *S) TestSaveCurrentAreasAsLayout() {
	reg := FocusAreas{
		Areas:       []string{"A", "B", "C", "D"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	got, idx, err := SaveCurrentAreasAsLayout(reg)
	s.NoError(err)
	s.Equal(1, idx)
	s.Equal([][]int{{0, 1, 2}, {0, 1, 2, 3}}, got.AreaLayouts)
	s.Equal(uint8(1), got.LastUsedAreaLayoutIndex)
}

func (s *S) TestSaveCurrentAreasAsLayoutNeedsThreeAreas() {
	reg := FocusAreas{
		Areas:       []string{"A", "B"},
		AreaLayouts: [][]int{},
	}
	_, _, err := SaveCurrentAreasAsLayout(reg)
	s.Error(err)
	s.Contains(err.Error(), "at least 3 areas")
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
	s.Contains(got, "# Area")
	s.Contains(got, "|   日 |   月 |   火 |   水 |   木 |   金 |   土")
	s.Contains(got, "Deep Work  |")
	s.Contains(got, "120")
	s.Contains(got, "90")
	s.Contains(got, "Exercise")
}

func (s *S) TestRenderWeekBufferFixedAreaColumnWidth() {
	week := WeekValues{
		Areas: []int{0, 1},
		Values: [][]*int{
			make([]*int, 7),
			make([]*int, 7),
		},
	}
	areaNames := []string{"A", "運動"}
	weekStart := time.Date(2025, 3, 3, 0, 0, 0, 0, time.UTC)
	got := RenderWeekBuffer(week, areaNames, 2025, 9, weekStart)
	expectedWidth := runewidth.StringWidth("運動") + 1

	for _, line := range strings.Split(got, "\n") {
		if strings.HasPrefix(line, "A ") || strings.HasPrefix(line, "運動") {
			left := strings.SplitN(line, "|", 2)[0]
			s.Equal(expectedWidth+1, runewidth.StringWidth(left))
		}
	}
}

func (s *S) TestRenderWeekBufferAlignsUsingLongestAreaName() {
	week := WeekValues{
		Areas: []int{0, 1, 2},
		Values: [][]*int{
			make([]*int, 7),
			make([]*int, 7),
			make([]*int, 7),
		},
	}
	areaNames := []string{
		"短い",
		"かなり長いエリア名です",
		"mid",
	}
	weekStart := time.Date(2025, 3, 3, 0, 0, 0, 0, time.UTC)
	got := RenderWeekBuffer(week, areaNames, 2025, 9, weekStart)
	expectedAreaColWidth := runewidth.StringWidth("かなり長いエリア名です") + 1

	var widths []int
	var headerPipeWidths []int
	var rowPipeWidths []int
	pipeDisplayWidths := func(line string) []int {
		widths := []int{}
		for i := 0; i < len(line); i++ {
			if line[i] == '|' {
				widths = append(widths, runewidth.StringWidth(line[:i]))
			}
		}
		return widths
	}
	for _, line := range strings.Split(got, "\n") {
		if strings.HasPrefix(line, "# ") &&
			strings.Contains(line, "|   日 |   月 |   火 |   水 |   木 |   金 |   土") {
			headerPipeWidths = pipeDisplayWidths(line)
		}
		if strings.HasPrefix(line, "短い") ||
			strings.HasPrefix(line, "かなり長いエリア名です") ||
			strings.HasPrefix(line, "mid") {
			left := strings.SplitN(line, "|", 2)[0]
			widths = append(widths, runewidth.StringWidth(left))
			if len(rowPipeWidths) == 0 {
				rowPipeWidths = pipeDisplayWidths(line)
			}
		}
	}
	s.Len(widths, 3)
	s.Equal(widths[0], widths[1])
	s.Equal(widths[1], widths[2])
	s.Equal(expectedAreaColWidth+1, widths[0]) // +1 for " " before "|"
	s.Equal(rowPipeWidths, headerPipeWidths)
}

func (s *S) TestParseWeekBuffer() {
	buf := `# focustime week view
# Week index: 2025w10 (Week starting: 2025-03-03)
# Columns: Area | 日 | 月 | 火 | 水 | 木 | 金 | 土

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

// TestParseAreasEditBuffer protects the loose edit UX contract:
// users can write one area per line with optional commas, comments,
// and wrapper lines while preserving area order after parse.
func (s *S) TestParseAreasEditBuffer() {
	buf := `# focustime areas edit
[
  Deep Work,
Coding

# ignored comment
 Exercise  ,
]
`
	got, err := ParseAreasEditBuffer([]byte(buf))
	s.NoError(err)
	s.Equal([]string{"Deep Work", "Coding", "Exercise"}, got)
}

// TestParseAreasEditBufferEmptyItem enforces that blank entries are rejected
// so edits never silently create empty area names in areas.json.
func (s *S) TestParseAreasEditBufferEmptyItem() {
	buf := `[
Deep Work,
  ,
Coding
]`
	_, err := ParseAreasEditBuffer([]byte(buf))
	s.Error(err)
	s.Contains(err.Error(), "area name cannot be empty")
}

func (s *S) TestRenderAreasEditBuffer() {
	got := RenderAreasEditBuffer([]string{"A", "B"})
	s.Contains(got, "# focustime areas edit")
	s.Contains(got, "[")
	s.Contains(got, "A,")
	s.Contains(got, "B,")
	s.Contains(got, "]")
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

func (s *S) TestWeekdayToDayIndex() {
	s.Equal(0, WeekdayToDayIndex(time.Sunday))
	s.Equal(1, WeekdayToDayIndex(time.Monday))
	s.Equal(2, WeekdayToDayIndex(time.Tuesday))
	s.Equal(3, WeekdayToDayIndex(time.Wednesday))
	s.Equal(4, WeekdayToDayIndex(time.Thursday))
	s.Equal(5, WeekdayToDayIndex(time.Friday))
	s.Equal(6, WeekdayToDayIndex(time.Saturday))
}

func (s *S) TestLogTime() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{},
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	logged, err := LogTime(cfg, "45m", 1)
	s.NoError(err)
	s.Equal(45, logged)

	now := time.Now().UTC()
	woy := TimeToWoY(now)
	yf, err := LoadYearFile(cfg, woy.Year)
	s.NoError(err)
	week := yf.Weeks[woy.WeekIndex()]
	s.NotNil(week)
	row := findAreaRowIndex(week, 1)
	s.NotEqual(-1, row)
	day := WeekdayToDayIndex(now.Weekday())
	s.NotNil(week.Values[row][day])
	s.Equal(45, *week.Values[row][day])

	// second log accumulates
	_, err = LogTime(cfg, "15m", 1)
	s.NoError(err)
	yf2, err := LoadYearFile(cfg, woy.Year)
	s.NoError(err)
	week2 := yf2.Weeks[woy.WeekIndex()]
	row2 := findAreaRowIndex(week2, 1)
	s.NotNil(week2.Values[row2][day])
	s.Equal(60, *week2.Values[row2][day])
}

func (s *S) TestTodayReportNoWeekData() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	now := time.Date(2026, 3, 3, 12, 0, 0, 0, time.UTC)
	out, err := TodayReport(cfg, now)
	s.NoError(err)
	s.Contains(out, "Today (2026-03-03, Tuesday): no data yet.")
}

func (s *S) TestTodayReportWeekExistsButNoTodayData() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	now := time.Date(2026, 3, 3, 12, 0, 0, 0, time.UTC) // Tuesday
	woy := TimeToWoY(now)
	yf := emptyYearFile(woy.Year)
	week := &WeekValues{
		Areas: []int{0, 1, 2},
		Values: [][]*int{
			make([]*int, 7),
			make([]*int, 7),
			make([]*int, 7),
		},
	}
	v := 60
	week.Values[0][WeekdayToDayIndex(time.Monday)] = &v // not today
	yf.Weeks[woy.WeekIndex()] = week
	err = SaveYearFile(cfg, yf)
	s.NoError(err)

	out, err := TodayReport(cfg, now)
	s.NoError(err)
	s.Contains(out, "Today (2026-03-03, Tuesday)")
	s.Contains(out, "No data logged for today.")
}

func (s *S) TestTodayReportWithValuesAndTotal() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	now := time.Date(2026, 3, 3, 12, 0, 0, 0, time.UTC) // Tuesday
	woy := TimeToWoY(now)
	yf := emptyYearFile(woy.Year)
	week := &WeekValues{
		Areas: []int{0, 1, 2},
		Values: [][]*int{
			make([]*int, 7),
			make([]*int, 7),
			make([]*int, 7),
		},
	}
	dayIdx := WeekdayToDayIndex(now.Weekday())
	v1, v2 := 30, 45
	week.Values[0][dayIdx] = &v1
	week.Values[2][dayIdx] = &v2
	yf.Weeks[woy.WeekIndex()] = week
	err = SaveYearFile(cfg, yf)
	s.NoError(err)

	out, err := TodayReport(cfg, now)
	s.NoError(err)
	s.Contains(out, "Today (2026-03-03, Tuesday)")
	s.Contains(out, "0 → A: 30m")
	s.Contains(out, "2 → C: 45m")
	s.Contains(out, "Total: 75m")
}

func (s *S) TestCurrentWeekReportNoWeekData() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	now := time.Date(2026, 3, 3, 12, 0, 0, 0, time.UTC)
	out, err := CurrentWeekReport(cfg, now)
	s.NoError(err)
	s.Contains(out, "No data for week")
}

func (s *S) TestCurrentWeekReportWithWeekData() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	now := time.Date(2026, 3, 3, 12, 0, 0, 0, time.UTC)
	woy := TimeToWoY(now)
	yf := emptyYearFile(woy.Year)
	week := &WeekValues{
		Areas: []int{0, 1, 2},
		Values: [][]*int{
			make([]*int, 7),
			make([]*int, 7),
			make([]*int, 7),
		},
	}
	dayIdx := WeekdayToDayIndex(now.Weekday())
	v := 50
	week.Values[1][dayIdx] = &v
	yf.Weeks[woy.WeekIndex()] = week
	err = SaveYearFile(cfg, yf)
	s.NoError(err)

	out, err := CurrentWeekReport(cfg, now)
	s.NoError(err)
	s.Contains(out, "# focustime week view")
	s.Contains(out, "# Week index:")
	s.Contains(out, "A     |")
	s.Contains(out, "B     |")
	s.Contains(out, "C     |")
	s.Contains(out, "50")
}

func (s *S) TestLogTimeErrors() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	// Invalid area index paths
	reg := FocusAreas{
		Areas:       []string{"A", "B", "C"},
		AreaLayouts: [][]int{{0, 1, 2}},
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	_, err = LogTime(cfg, "45m", -1)
	s.Error(err)
	_, err = LogTime(cfg, "45m", 99)
	s.Error(err)

	// Invalid duration paths
	_, err = LogTime(cfg, "bad", 1)
	s.Error(err)
	_, err = LogTime(cfg, "30s", 1)
	s.Error(err)
}

func (s *S) TestLogTimeBootstrapFailure() {
	newHome := s.T().TempDir()
	s.T().Setenv("XDG_DATA_HOME", newHome)
	cfg := StartCfg{HomeDir: newHome}

	// Fewer than 3 areas and no layouts/weeks -> GetDefaultAreaLayout error
	reg := FocusAreas{
		Areas:       []string{"A", "B"},
		AreaLayouts: [][]int{},
	}
	err := SaveAreasFile(cfg, reg)
	s.NoError(err)

	_, err = LogTime(cfg, "45m", 1)
	s.Error(err)
	s.Contains(err.Error(), "no area layout")
}
