package src

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/mattn/go-runewidth"
)

// StartCfg holds startup configuration. HomeDir is optional: when empty,
// os.UserHomeDir is used for XDG fallbacks. Timezone is an IANA name
// (persisted in config.json). Loc is derived from Timezone for runtime; if set
// with Loc != nil, ResolveStartCfg skips file/timezone resolution (tests).
type StartCfg struct {
	HomeDir  string         `json:"-"`
	Timezone string         `json:"timezone"`
	Loc      *time.Location `json:"-"`
}

// EffectiveHomeDir returns HomeDir or os.UserHomeDir when HomeDir is empty.
func EffectiveHomeDir(cfg StartCfg) (string, error) {
	if cfg.HomeDir != "" {
		return cfg.HomeDir, nil
	}
	return os.UserHomeDir()
}

func fillEffectiveHomeDir(cfg *StartCfg) error {
	if cfg.HomeDir != "" {
		return nil
	}
	h, err := os.UserHomeDir()
	if err != nil {
		return fmt.Errorf("resolve home directory: %w", err)
	}
	cfg.HomeDir = h
	return nil
}

// MergeTimezone returns raw if non-empty, else America/Chicago.
func MergeTimezone(raw string) string {
	if raw != "" {
		return raw
	}
	return "America/Chicago"
}

// ValidateAndLocateTimezone loads an IANA timezone name.
func ValidateAndLocateTimezone(tz string) (*time.Location, error) {
	if tz == "" {
		return nil, fmt.Errorf("timezone is empty")
	}
	loc, err := time.LoadLocation(tz)
	if err != nil {
		return nil, err
	}
	return loc, nil
}

// ConfigFilePath returns the path to focustime config.json under XDG_CONFIG_HOME.
func ConfigFilePath(cfg StartCfg) (string, error) {
	c := cfg
	if err := fillEffectiveHomeDir(&c); err != nil {
		return "", err
	}
	xdg, err := XdgConfigHome(c)
	if err != nil {
		return "", err
	}
	return filepath.Join(xdg, "focustime", "config.json"), nil
}

// readMergedTimezoneFromDisk returns the effective timezone string from
// config.json or default. Missing file is not an error.
func readMergedTimezoneFromDisk(cfg StartCfg) (string, error) {
	path, err := ConfigFilePath(cfg)
	if err != nil {
		return "", err
	}
	b, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return MergeTimezone(""), nil
		}
		return "", err
	}
	var s StartCfg
	if err := json.Unmarshal(b, &s); err != nil {
		return "", fmt.Errorf("config file: invalid JSON: %w", err)
	}
	return MergeTimezone(s.Timezone), nil
}

// ResolveStartCfg fills HomeDir when empty, then resolves Loc from Loc (passthrough),
// Timezone (caller override), or config.json / default Chicago.
func ResolveStartCfg(base StartCfg) (StartCfg, error) {
	copy := base
	if err := fillEffectiveHomeDir(&copy); err != nil {
		return StartCfg{}, err
	}
	if copy.Loc != nil {
		return copy, nil
	}
	if copy.Timezone != "" {
		loc, err := ValidateAndLocateTimezone(copy.Timezone)
		if err != nil {
			return StartCfg{}, fmt.Errorf("timezone %q: %w", copy.Timezone, err)
		}
		return StartCfg{
			HomeDir:  copy.HomeDir,
			Timezone: copy.Timezone,
			Loc:      loc,
		}, nil
	}
	tz, err := readMergedTimezoneFromDisk(copy)
	if err != nil {
		return StartCfg{}, err
	}
	loc, err := ValidateAndLocateTimezone(tz)
	if err != nil {
		return StartCfg{}, fmt.Errorf("config timezone %q: %w", tz, err)
	}
	return StartCfg{
		HomeDir:  copy.HomeDir,
		Timezone: tz,
		Loc:      loc,
	}, nil
}

// EffectiveConfigJSON returns the on-disk or default timezone as pretty JSON
// for `focustime config` (merged effective values only).
func EffectiveConfigJSON(cfg StartCfg) ([]byte, error) {
	c := cfg
	if err := fillEffectiveHomeDir(&c); err != nil {
		return nil, err
	}
	tz, err := readMergedTimezoneFromDisk(c)
	if err != nil {
		return nil, err
	}
	out := StartCfg{Timezone: tz}
	return json.MarshalIndent(out, "", "  ")
}

// SaveStartCfgFile writes timezone to config.json atomically after validation.
func SaveStartCfgFile(cfg StartCfg, toSave StartCfg) error {
	tz := MergeTimezone(toSave.Timezone)
	if _, err := ValidateAndLocateTimezone(tz); err != nil {
		return fmt.Errorf("invalid timezone %q: %w", tz, err)
	}
	path, err := ConfigFilePath(cfg)
	if err != nil {
		return err
	}
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	payload := StartCfg{Timezone: tz}
	jsonBz, err := json.MarshalIndent(payload, "", "  ")
	if err != nil {
		return err
	}
	tmpPath := filepath.Join(dir, fmt.Sprintf("config.json.%d.tmp", os.Getpid()))
	if err := os.WriteFile(tmpPath, jsonBz, 0o644); err != nil {
		return err
	}
	return os.Rename(tmpPath, path)
}

// ConfigEdit opens config.json in $EDITOR; only valid JSON and timezone persist.
func ConfigEdit(cfg StartCfg) error {
	c := cfg
	if err := fillEffectiveHomeDir(&c); err != nil {
		return err
	}
	path, err := ConfigFilePath(c)
	if err != nil {
		return err
	}
	var seed []byte
	if b, err := os.ReadFile(path); err == nil {
		var s StartCfg
		if json.Unmarshal(b, &s) == nil {
			s.Timezone = MergeTimezone(s.Timezone)
			seed, err = json.MarshalIndent(StartCfg{Timezone: s.Timezone}, "", "  ")
			if err != nil {
				return err
			}
			seed = append(seed, '\n')
		} else {
			seed = append([]byte(nil), b...)
		}
	} else if os.IsNotExist(err) {
		seed, err = json.MarshalIndent(StartCfg{Timezone: MergeTimezone("")}, "", "  ")
		if err != nil {
			return err
		}
		seed = append(seed, '\n')
	} else {
		return err
	}

	tmp, err := os.CreateTemp("", "focustime-config_*")
	if err != nil {
		return err
	}
	tempPath := tmp.Name()
	defer os.Remove(tempPath)
	if _, err := tmp.Write(seed); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := LaunchEditor(tempPath); err != nil {
		return fmt.Errorf("editor exited with error: %w", err)
	}
	fileBz, err := os.ReadFile(tempPath)
	if err != nil {
		return err
	}
	var parsed StartCfg
	if err := json.Unmarshal(fileBz, &parsed); err != nil {
		return fmt.Errorf("invalid JSON: %w", err)
	}
	return SaveStartCfgFile(c, parsed)
}

// Location returns Loc or UTC when Loc is nil.
func (c StartCfg) Location() *time.Location {
	if c.Loc != nil {
		return c.Loc
	}
	return time.UTC
}

// FocusAreas is the areas.json schema: display names, saved layouts,
// and the last-used layout index for new weeks.
type FocusAreas struct {
	Areas                   []string `json:"areas"`
	AreaLayouts             [][]int  `json:"area_layouts"`
	LastUsedAreaLayoutIndex uint8    `json:"last_used_area_layout_index"`
}

// LoadAreasFile loads the areas.json file from the focustime data directory,
// creating the directory and file if they do not already exist.
func LoadAreasFile(cfg StartCfg) (FocusAreas, error) {
	fresh := FocusAreas{
		Areas:                   []string{},
		AreaLayouts:             [][]int{},
		LastUsedAreaLayoutIndex: 0,
	}
	xdgDataHome, err := XdgDataHome(cfg)
	if err != nil {
		return fresh, err
	}
	fileInfo, err := os.Stat(xdgDataHome)
	if err != nil && os.IsNotExist(err) {
		err := os.MkdirAll(xdgDataHome, 0o755)
		return fresh, err
	} else if err != nil {
		return fresh, err
	}

	if !fileInfo.IsDir() {
		return fresh, fmt.Errorf("file exists in place of expected directory at %s", xdgDataHome)
	}

	err = CreateFocusTimeDir(cfg)
	if err != nil {
		return fresh, err
	}

	dir, err := DirFTData(cfg)
	if err != nil {
		return fresh, err
	}
	fp := filepath.Join(dir, "areas.json")
	fileInfo, err = os.Stat(fp)
	switch {
	case err != nil && os.IsNotExist(err):

		jsonBz, _ := json.MarshalIndent(fresh, "", "  ")
		err = os.WriteFile(fp, jsonBz, 0o644)
		return fresh, err
	case err != nil:
		return fresh, err
	}

	fileBz, err := os.ReadFile(fp)
	if err != nil {
		return fresh, err
	}

	var reg FocusAreas
	err = json.Unmarshal(fileBz, &reg)
	if err != nil {
		return fresh, err
	}

	return reg, nil
}

// SaveAreasFile writes reg to areas.json atomically (write to temp, rename).
func SaveAreasFile(cfg StartCfg, reg FocusAreas) error {
	if err := CreateFocusTimeDir(cfg); err != nil {
		return err
	}
	dir, err := DirFTData(cfg)
	if err != nil {
		return err
	}
	jsonBz, err := json.MarshalIndent(reg, "", "  ")
	if err != nil {
		return err
	}
	tmpPath := filepath.Join(dir, fmt.Sprintf("areas.json.%d.tmp", os.Getpid()))
	if err := os.WriteFile(tmpPath, jsonBz, 0o644); err != nil {
		return err
	}
	destPath := filepath.Join(dir, "areas.json")
	return os.Rename(tmpPath, destPath)
}

// AddArea appends name to reg.Areas and returns the updated registry.
// No I/O; caller must save.
func AddArea(reg FocusAreas, name string) FocusAreas {
	reg.Areas = append(reg.Areas, name)
	return reg
}

// SaveCurrentAreasAsLayout appends the current areas order (0..n-1) as a saved
// layout and marks it as last used. Requires at least 3 areas.
func SaveCurrentAreasAsLayout(reg FocusAreas) (FocusAreas, int, error) {
	if len(reg.Areas) < 3 {
		return reg, -1, fmt.Errorf("need at least 3 areas to save layout")
	}
	layout := make([]int, len(reg.Areas))
	for i := range reg.Areas {
		layout[i] = i
	}
	reg.AreaLayouts = append(reg.AreaLayouts, layout)
	newIndex := len(reg.AreaLayouts) - 1
	reg.LastUsedAreaLayoutIndex = uint8(newIndex)
	return reg, newIndex, nil
}

// RemoveArea removes the area at index from reg.Areas, updates area_layouts
// (removes the ID and decrements IDs > index), and adjusts last_used_area_layout_index.
// Returns error if index is out of range.
func RemoveArea(reg FocusAreas, index int) (FocusAreas, error) {
	n := len(reg.Areas)
	if index < 0 || index >= n {
		return reg, fmt.Errorf("area index %d out of range [0, %d)", index, n)
	}
	newAreas := make([]string, 0, n-1)
	newAreas = append(newAreas, reg.Areas[:index]...)
	newAreas = append(newAreas, reg.Areas[index+1:]...)
	reg.Areas = newAreas

	// Update area_layouts: remove index, decrement IDs > index; drop empty layouts
	var newLayouts [][]int
	for _, layout := range reg.AreaLayouts {
		var updated []int
		for _, id := range layout {
			if id == index {
				continue
			}
			if id > index {
				updated = append(updated, id-1)
			} else {
				updated = append(updated, id)
			}
		}
		if len(updated) > 0 {
			newLayouts = append(newLayouts, updated)
		}
	}
	reg.AreaLayouts = newLayouts

	// Adjust last_used_area_layout_index
	lu := int(reg.LastUsedAreaLayoutIndex)
	if lu >= len(reg.AreaLayouts) {
		reg.LastUsedAreaLayoutIndex = 0
	}
	return reg, nil
}

// CreateFocusTimeDir: Creates the primary data directory if it does not already
// exist. Otherwise, does nothing.
func CreateFocusTimeDir(cfg StartCfg) error {
	ftDir, err := DirFTData(cfg)
	if err != nil {
		return err
	}
	info, err := os.Stat(ftDir)
	switch {
	case err == nil && info.IsDir():
		return nil
	case err == nil && !info.IsDir():
		return fmt.Errorf("File exists in place of expected path for the focustime data dir, %q", ftDir)
	case err != nil && os.IsNotExist(err):
		// continue
	default:
		return err // Something strange happened
	}

	err = os.MkdirAll(ftDir, 0o755)
	return err
}

// XdgConfigHome returns the XDG configuration directory, falling back
// to $HOME/.config when XDG_CONFIG_HOME is not set.
func XdgConfigHome(cfg StartCfg) (string, error) {
	if v := os.Getenv("XDG_CONFIG_HOME"); v != "" {
		return v, nil
	}
	home, err := EffectiveHomeDir(cfg)
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".config"), nil
}

// XdgDataHome returns the XDG data directory, falling back to
// $HOME/.local/share when XDG_DATA_HOME is not set.
// Data holds persistent downloaded assets.
func XdgDataHome(cfg StartCfg) (string, error) {
	if v := os.Getenv("XDG_DATA_HOME"); v != "" {
		return v, nil
	}
	home, err := EffectiveHomeDir(cfg)
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".local", "share"), nil
}

// XdgStateHome returns the XDG state directory, falling back to
// $HOME/.local/state when XDG_STATE_HOME is not set.
// State in the XDG spec is runtime info that persists but is not user-editable.
// It typically holds session state like logs and runtime history.
func XdgStateHome(cfg StartCfg) (string, error) {
	if v := os.Getenv("XDG_STATE_HOME"); v != "" {
		return v, nil
	}
	home, err := EffectiveHomeDir(cfg)
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".local", "state"), nil
}

// XdgCacheHome returns the XDG cache directory, falling back to $HOME/.cache
// when XDG_CACHE_HOME is not set. Cache is used for ephemeral speed-ups. Cache
// is meant to be safe to delete at all times.
func XdgCacheHome(cfg StartCfg) (string, error) {
	if v := os.Getenv("XDG_CACHE_HOME"); v != "" {
		return v, nil
	}
	home, err := EffectiveHomeDir(cfg)
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".cache"), nil
}

// DirFTData returns the "$XDG_DATA_HOME/focustime" directory.
func DirFTData(cfg StartCfg) (string, error) {
	d, err := XdgDataHome(cfg)
	if err != nil {
		return "", err
	}
	return filepath.Join(d, "focustime"), nil
}

// WeekValues holds one week's data. Areas is the ordered list of area IDs
// (rows). Values is [areaIdx][dayIdx], Sun=0..Sat=6; nil = unset.
type WeekValues struct {
	Areas  []int    `json:"areas"`
	Values [][]*int `json:"values"`
}

// YearFile holds time-series data for one calendar year.
// Weeks index i (0-based) = ISO week i+1; nil = no data.
type YearFile struct {
	Year    int             `json:"year"`
	Version int             `json:"version"`
	Weeks   [53]*WeekValues `json:"weeks"`
}

// WoY identifies an ISO week by year and week number (1-53).
type WoY struct {
	Year int
	Week int
}

// FormatYYYYWW returns "YYYY-WW" (e.g. "2025-10") for display.
func (w WoY) FormatYYYYWW() string {
	return fmt.Sprintf("%d-%02d", w.Year, w.Week)
}

// WeekIndex returns the 0-based index into YearFile.Weeks (week - 1).
func (w WoY) WeekIndex() int {
	return w.Week - 1
}

// ParseYearWWeek parses "{year}w{week}" (e.g. "2026w1") into year and week.
// Week must be 1-53. Case-insensitive for the "w".
func ParseYearWWeek(s string) (year, week int, err error) {
	lower := strings.ToLower(s)
	i := strings.Index(lower, "w")
	if i < 1 || i >= len(s)-1 {
		return 0, 0, fmt.Errorf("invalid YYYYwWW format: %q (expected e.g. 2026w1)", s)
	}
	year, err = strconv.Atoi(s[:i])
	if err != nil {
		return 0, 0, fmt.Errorf("invalid year in %q: %w", s, err)
	}
	week, err = strconv.Atoi(s[i+1:])
	if err != nil {
		return 0, 0, fmt.Errorf("invalid week in %q: %w", s, err)
	}
	if week < 1 || week > 53 {
		return 0, 0, fmt.Errorf("week must be 1-53, got %d", week)
	}
	return year, week, nil
}

// TimeToWoY converts t to its ISO week-of-year in loc and returns the
// corresponding WoY value. A nil loc is treated as UTC.
func TimeToWoY(t time.Time, loc *time.Location) WoY {
	if loc == nil {
		loc = time.UTC
	}
	t = t.In(loc)
	year, weekOfYear := t.ISOWeek()
	return WoY{
		Year: year,
		Week: weekOfYear,
	}
}

// ParseDurationToMinutes parses durations like "2h", "45m", "1.5h"
// and returns whole minutes.
func ParseDurationToMinutes(s string) (int, error) {
	d, err := time.ParseDuration(s)
	if err != nil {
		return 0, fmt.Errorf("invalid duration %q: %w", s, err)
	}
	if d <= 0 {
		return 0, fmt.Errorf("duration must be positive, got %q", s)
	}
	if d%time.Minute != 0 {
		return 0, fmt.Errorf("duration must be in whole minutes, got %q", s)
	}
	return int(d / time.Minute), nil
}

var weekdayLabelsJP = [7]string{"日", "月", "火", "水", "木", "金", "土"}

// WeekdayToDayIndex maps a Go weekday to Sun=0..Sat=6.
func WeekdayToDayIndex(wd time.Weekday) int {
	return int(wd)
}

func resolveAreaNames(reg FocusAreas, areaIDs []int) []string {
	areaNames := make([]string, len(areaIDs))
	for i, id := range areaIDs {
		if id >= 0 && id < len(reg.Areas) {
			areaNames[i] = reg.Areas[id]
		} else {
			areaNames[i] = fmt.Sprintf("Area %d", id)
		}
	}
	return areaNames
}

func findAreaRowIndex(week *WeekValues, areaID int) int {
	for i, id := range week.Areas {
		if id == areaID {
			return i
		}
	}
	return -1
}

func ensureAreaRow(week *WeekValues, areaID int) int {
	row := findAreaRowIndex(week, areaID)
	if row >= 0 {
		return row
	}
	week.Areas = append(week.Areas, areaID)
	week.Values = append(week.Values, make([]*int, 7))
	return len(week.Areas) - 1
}

// LogTime adds minutes to today's cell for areaID in the current week.
func LogTime(cfg StartCfg, durationStr string, areaID int) (int, error) {
	minutes, err := ParseDurationToMinutes(durationStr)
	if err != nil {
		return 0, err
	}
	reg, err := LoadAreasFile(cfg)
	if err != nil {
		return 0, err
	}
	if areaID < 0 || areaID >= len(reg.Areas) {
		return 0, fmt.Errorf("area index %d out of range [0, %d)",
			areaID, len(reg.Areas))
	}
	loc := cfg.Location()
	now := time.Now().In(loc)
	woy := TimeToWoY(now, loc)
	yf, err := LoadYearFile(cfg, woy.Year)
	if err != nil {
		return 0, err
	}
	defaultAreas, err := GetDefaultAreaLayout(cfg, reg, &yf)
	if err != nil {
		return 0, err
	}
	week, err := FindOrCreateWeek(&yf, woy.WeekIndex(), defaultAreas)
	if err != nil {
		return 0, err
	}
	row := ensureAreaRow(week, areaID)
	dayIdx := WeekdayToDayIndex(now.Weekday())
	if week.Values[row][dayIdx] == nil {
		week.Values[row][dayIdx] = new(int)
	}
	*week.Values[row][dayIdx] += minutes
	if err := SaveYearFile(cfg, yf); err != nil {
		return 0, err
	}
	return minutes, nil
}

// CurrentWeekReport returns a table for the current week.
func CurrentWeekReport(cfg StartCfg, now time.Time) (string, error) {
	reg, err := LoadAreasFile(cfg)
	if err != nil {
		return "", err
	}
	now = now.In(cfg.Location())
	woy := TimeToWoY(now, cfg.Location())
	yf, err := LoadYearFile(cfg, woy.Year)
	if err != nil {
		return "", err
	}
	week := yf.Weeks[woy.WeekIndex()]
	if week == nil {
		return fmt.Sprintf("No data for week %dw%d.\n", woy.Year, woy.Week),
			nil
	}
	areaNames := resolveAreaNames(reg, week.Areas)
	weekStart := weekStartFor(woy.Year, woy.Week)
	return RenderWeekBuffer(*week, areaNames, woy.Year, woy.WeekIndex(),
		weekStart, now), nil
}

// TodayReport returns today's logged values by area for the current week.
func TodayReport(cfg StartCfg, now time.Time) (string, error) {
	reg, err := LoadAreasFile(cfg)
	if err != nil {
		return "", err
	}
	now = now.In(cfg.Location())
	woy := TimeToWoY(now, cfg.Location())
	yf, err := LoadYearFile(cfg, woy.Year)
	if err != nil {
		return "", err
	}
	week := yf.Weeks[woy.WeekIndex()]
	dayIdx := WeekdayToDayIndex(now.Weekday())
	dayLabel := now.Weekday().String()
	dateLabel := now.Format("2006-01-02")
	timeLabel := now.Format("15:04:05 MST")
	if week == nil {
		return fmt.Sprintf("Today (%s, %s, %s): no data yet.\n",
			dateLabel, dayLabel, timeLabel), nil
	}
	areaNames := resolveAreaNames(reg, week.Areas)
	var b strings.Builder
	b.WriteString(fmt.Sprintf("Today (%s, %s, %s)\n", dateLabel, dayLabel, timeLabel))
	hasData := false
	total := 0
	for row := range week.Areas {
		if row >= len(week.Values) || dayIdx >= len(week.Values[row]) {
			continue
		}
		v := week.Values[row][dayIdx]
		if v == nil {
			continue
		}
		hasData = true
		total += *v
		b.WriteString(fmt.Sprintf("%d → %s: %dm\n",
			week.Areas[row], areaNames[row], *v))
	}
	if !hasData {
		b.WriteString("No data logged for today.\n")
		return b.String(), nil
	}
	b.WriteString(fmt.Sprintf("Total: %dm\n", total))
	return b.String(), nil
}

// LoadYearFile loads the year file (e.g. 2025.json) from the data dir.
// If missing, returns an empty YearFile for that year.
func LoadYearFile(cfg StartCfg, year int) (YearFile, error) {
	fresh := emptyYearFile(year)
	if err := CreateFocusTimeDir(cfg); err != nil {
		return fresh, err
	}
	dir, err := DirFTData(cfg)
	if err != nil {
		return fresh, err
	}
	fp := filepath.Join(dir, fmt.Sprintf("%d.json", year))
	fileBz, err := os.ReadFile(fp)
	if err != nil {
		if os.IsNotExist(err) {
			return fresh, nil
		}
		return fresh, err
	}
	var yf YearFile
	if err := json.Unmarshal(fileBz, &yf); err != nil {
		return fresh, err
	}
	return yf, nil
}

// emptyYearFile returns a zeroed YearFile for the given year.
func emptyYearFile(year int) YearFile {
	return YearFile{
		Year:    year,
		Version: 1,
		Weeks:   [53]*WeekValues{},
	}
}

// SaveYearFile writes the year file atomically (temp + rename).
func SaveYearFile(cfg StartCfg, yf YearFile) error {
	if err := CreateFocusTimeDir(cfg); err != nil {
		return err
	}
	dir, err := DirFTData(cfg)
	if err != nil {
		return err
	}
	jsonBz, err := json.MarshalIndent(yf, "", "  ")
	if err != nil {
		return err
	}
	tmpPath := filepath.Join(dir, fmt.Sprintf("%d.json.%d.tmp", yf.Year, os.Getpid()))
	if err := os.WriteFile(tmpPath, jsonBz, 0o644); err != nil {
		return err
	}
	destPath := filepath.Join(dir, fmt.Sprintf("%d.json", yf.Year))
	return os.Rename(tmpPath, destPath)
}

// GetDefaultAreaLayout returns the default area IDs for a new week.
// Precedence: last_used layout → most recent week's areas → bootstrap fallback → error.
func GetDefaultAreaLayout(cfg StartCfg, reg FocusAreas, yearFile *YearFile) ([]int, error) {
	// 1. last_used_area_layout_index
	idx := int(reg.LastUsedAreaLayoutIndex)
	if idx >= 0 && idx < len(reg.AreaLayouts) {
		layout := reg.AreaLayouts[idx]
		if len(layout) > 0 && validAreaLayout(layout, len(reg.Areas)) {
			return append([]int(nil), layout...), nil
		}
	}
	// 2. Most recent week in yearFile
	for i := 52; i >= 0; i-- {
		if yearFile != nil && yearFile.Weeks[i] != nil {
			areas := yearFile.Weeks[i].Areas
			if len(areas) > 0 {
				return append([]int(nil), areas...), nil
			}
		}
	}
	// 3. Bootstrap fallback: first 3 areas when 3+ exist
	if len(reg.Areas) >= 3 {
		return []int{0, 1, 2}, nil
	}
	// 4. Error
	return nil, fmt.Errorf("no area layout. Add at least 3 areas and run: focustime areas save-layout")
}

// validAreaLayout returns true if all IDs in layout are in range [0, nAreas).
func validAreaLayout(layout []int, nAreas int) bool {
	for _, id := range layout {
		if id < 0 || id >= nAreas {
			return false
		}
	}
	return true
}

// FindOrCreateWeek finds the week at weekIndex in yf, or creates it with
// defaultAreas and an empty values grid.
func FindOrCreateWeek(yf *YearFile, weekIndex int, defaultAreas []int) (*WeekValues, error) {
	if weekIndex < 0 || weekIndex >= 53 {
		return nil, fmt.Errorf("week index %d out of range [0, 53)", weekIndex)
	}
	if yf.Weeks[weekIndex] != nil {
		return yf.Weeks[weekIndex], nil
	}
	// Create empty values grid: one row per area, 7 columns
	values := make([][]*int, len(defaultAreas))
	for i := range values {
		values[i] = make([]*int, 7)
	}
	week := &WeekValues{
		Areas:  append([]int(nil), defaultAreas...),
		Values: values,
	}
	yf.Weeks[weekIndex] = week
	return week, nil
}

// RenderWeekBuffer produces the week view text format per spec §4.1.
func RenderWeekBuffer(week WeekValues, areaNames []string, year, weekIndex int,
	weekStart time.Time, nowLocal time.Time) string {
	areaIDs := make([]string, len(week.Areas))
	for i, id := range week.Areas {
		areaIDs[i] = fmt.Sprint(id)
	}
	areaIDList := strings.Join(areaIDs, ", ")
	weekNum := weekIndex + 1

	maxAreaWidth := runewidth.StringWidth("Area")
	for _, name := range areaNames {
		if w := runewidth.StringWidth(name); w > maxAreaWidth {
			maxAreaWidth = w
		}
	}
	areaColWidth := maxAreaWidth + 1
	const numWidth = 4

	var b strings.Builder
	b.WriteString("# focustime week view\n")
	b.WriteString("# Week index: " + fmt.Sprintf("%dw%d", year, weekNum) + " (Week starting: " + weekStart.Format("2006-01-02") + ")\n")
	b.WriteString("# Areas (row order): " + areaIDList + "\n")
	b.WriteString("# Units: minutes\n")
	b.WriteString("# Edit numeric cells only. Empty = unset.\n")
	b.WriteString("#\n")
	const headerPrefix = "# "
	b.WriteString(headerPrefix)
	headerAreaColWidth := areaColWidth - runewidth.StringWidth(headerPrefix)
	if headerAreaColWidth < runewidth.StringWidth("Area") {
		headerAreaColWidth = runewidth.StringWidth("Area")
	}
	b.WriteString(padRightDisplayWidth("Area", headerAreaColWidth))
	b.WriteString(" |")
	todayIdx, hasTodayMarker := headerTodayMarkerDayIndex(year, weekIndex, nowLocal)
	for i, day := range weekdayLabelsJP {
		label := day
		if hasTodayMarker && i == todayIdx {
			label = "🟢" + day
		}
		b.WriteString(" ")
		b.WriteString(padLeftDisplayWidth(label, numWidth))
		if i < len(weekdayLabelsJP)-1 {
			b.WriteString(" |")
		}
	}
	b.WriteString("\n")

	for row := 0; row < len(areaNames); row++ {
		name := areaNames[row]
		rowStr := padRightDisplayWidth(name, areaColWidth) + " |"
		vals := week.Values[row]
		for d := 0; d < len(weekdayLabelsJP); d++ {
			rowStr += " "
			if d < len(vals) && vals[d] != nil {
				rowStr += fmt.Sprintf("%*d", numWidth, *vals[d])
			} else {
				rowStr += strings.Repeat(" ", numWidth)
			}
			if d < 6 {
				rowStr += " |"
			}
		}
		b.WriteString(rowStr + "\n")
	}
	return b.String()
}

func headerTodayMarkerDayIndex(year int, weekIndex int, nowLocal time.Time) (int, bool) {
	if nowLocal.IsZero() {
		return 0, false
	}
	nowYear, nowWeek := nowLocal.ISOWeek()
	if nowYear != year || nowWeek != weekIndex+1 {
		return 0, false
	}
	return WeekdayToDayIndex(nowLocal.Weekday()), true
}

// padRightDisplayWidth pads s to at least width display cells with spaces.
func padRightDisplayWidth(s string, width int) string {
	w := runewidth.StringWidth(s)
	if w >= width {
		return s
	}
	return s + strings.Repeat(" ", width-w)
}

// padLeftDisplayWidth pads s on the left to at least width display cells.
func padLeftDisplayWidth(s string, width int) string {
	w := runewidth.StringWidth(s)
	if w >= width {
		return s
	}
	return strings.Repeat(" ", width-w) + s
}

// ParseWeekBuffer parses the edited buffer back into WeekValues.
// areaNames and areaIDs give the expected row order (display name and ID per row).
func ParseWeekBuffer(buf []byte, areaNames []string, areaIDs []int) (WeekValues, error) {
	lines := strings.Split(string(buf), "\n")
	seenHeader := false
	var dataRows [][]string
	for _, line := range lines {
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}
		parts := splitRow(line)
		if len(parts) < 8 {
			continue
		}
		if !seenHeader && strings.Contains(parts[0], "Columns") {
			seenHeader = true
			continue
		}
		dataRows = append(dataRows, parts)
	}
	if len(dataRows) != len(areaNames) {
		return WeekValues{}, fmt.Errorf("expected %d rows, got %d", len(areaNames), len(dataRows))
	}
	values := make([][]*int, len(areaNames))
	for i, parts := range dataRows {
		areaName := strings.TrimSpace(parts[0])
		if areaName != areaNames[i] {
			return WeekValues{}, fmt.Errorf("row %d: expected area %q, got %q", i+1, areaNames[i], areaName)
		}
		row := make([]*int, len(weekdayLabelsJP))
		for d := 0; d < len(weekdayLabelsJP); d++ {
			cell := strings.TrimSpace(parts[d+1])
			if cell == "" {
				row[d] = nil
				continue
			}
			v, err := strconv.Atoi(cell)
			if err != nil {
				return WeekValues{}, fmt.Errorf("row %d day %d: invalid number %q: %w", i+1, d+1, cell, err)
			}
			row[d] = &v
		}
		values[i] = row
	}
	return WeekValues{
		Areas:  append([]int(nil), areaIDs...),
		Values: values,
	}, nil
}

// splitRow splits a table row by | and returns the fields.
func splitRow(line string) []string {
	var parts []string
	start := 0
	for i := 0; i <= len(line); i++ {
		if i == len(line) || line[i] == '|' {
			parts = append(parts, strings.TrimSpace(line[start:i]))
			start = i + 1
		}
	}
	return parts
}

// ResolveEditor returns the editor command: EDITOR → nvim (if in PATH) → vi.
func ResolveEditor() string {
	if v := os.Getenv("EDITOR"); v != "" {
		return v
	}
	if _, err := exec.LookPath("nvim"); err == nil {
		return "nvim"
	}
	return "vi"
}

// LaunchEditor runs the resolved editor on tempPath, attaching stdio.
// Handles editor strings like "vim -f" by splitting on first space.
func LaunchEditor(tempPath string) error {
	raw := ResolveEditor()
	parts := strings.SplitN(raw, " ", 2)
	cmdName := parts[0]
	var args []string
	if len(parts) > 1 {
		args = append(strings.Fields(parts[1]), tempPath)
	} else {
		args = []string{tempPath}
	}
	cmd := exec.Command(cmdName, args...)
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

// RenderAreasEditBuffer renders areas as a loose array-like format.
func RenderAreasEditBuffer(areas []string) string {
	var b strings.Builder
	b.WriteString("# focustime areas edit\n")
	b.WriteString("# Edit area names (one per line). Spaces are allowed.\n")
	b.WriteString("# Lines starting with # are ignored.\n")
	b.WriteString("# Keep [ and ] wrapper lines; trailing commas are optional.\n")
	b.WriteString("[\n")
	for _, area := range areas {
		b.WriteString(area + ",\n")
	}
	b.WriteString("]\n")
	b.WriteString("# Example:\n")
	b.WriteString("# Deep Work,\n")
	b.WriteString("# Admin\n")
	return b.String()
}

// ParseAreasEditBuffer parses a loose array-like list of area names.
func ParseAreasEditBuffer(buf []byte) ([]string, error) {
	lines := strings.Split(string(buf), "\n")
	areas := make([]string, 0, len(lines))
	for i, raw := range lines {
		line := strings.TrimSpace(raw)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		if line == "[" || line == "]" {
			continue
		}
		if strings.HasSuffix(line, ",") {
			line = strings.TrimSpace(strings.TrimSuffix(line, ","))
		}
		if line == "" {
			return nil, fmt.Errorf("line %d: area name cannot be empty", i+1)
		}
		areas = append(areas, line)
	}
	return areas, nil
}

// AreasEdit opens areas.json areas in editor and saves edited values.
func AreasEdit(cfg StartCfg) error {
	reg, err := LoadAreasFile(cfg)
	if err != nil {
		return err
	}
	buf := RenderAreasEditBuffer(reg.Areas)

	tmp, err := os.CreateTemp("", "focustime-areas_*")
	if err != nil {
		return err
	}
	tempPath := tmp.Name()
	defer os.Remove(tempPath)

	if _, err := tmp.WriteString(buf); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := LaunchEditor(tempPath); err != nil {
		return fmt.Errorf("editor exited with error: %w", err)
	}
	fileBz, err := os.ReadFile(tempPath)
	if err != nil {
		return err
	}
	areas, err := ParseAreasEditBuffer(fileBz)
	if err != nil {
		return err
	}
	reg.Areas = areas
	return SaveAreasFile(cfg, reg)
}

// WeekEdit loads the week, opens it in $EDITOR, and saves after parse.
// Requires at least 3 areas and a resolvable default layout.
func WeekEdit(cfg StartCfg, woy WoY) error {
	// 1. Bootstrap
	reg, err := LoadAreasFile(cfg)
	if err != nil {
		return err
	}
	// 2. Guardrail: >= 3 areas
	if len(reg.Areas) < 3 {
		return fmt.Errorf("no areas defined. Add at least three areas first: " +
			"focustime areas add \"Deep Work\" (etc.)")
	}
	// 3. Load year file
	yf, err := LoadYearFile(cfg, woy.Year)
	if err != nil {
		return err
	}
	// 4. Default area layout
	defaultAreas, err := GetDefaultAreaLayout(cfg, reg, &yf)
	if err != nil {
		return err
	}
	// 5. Find or create week
	week, err := FindOrCreateWeek(&yf, woy.WeekIndex(), defaultAreas)
	if err != nil {
		return err
	}
	// 6. Resolve area names
	areaNames := make([]string, len(week.Areas))
	for i, id := range week.Areas {
		if id < len(reg.Areas) {
			areaNames[i] = reg.Areas[id]
		} else {
			areaNames[i] = fmt.Sprintf("Area %d", id)
		}
	}
	// 7. Compute week start (Monday of that ISO week)
	weekStart := weekStartFor(woy.Year, woy.Week)
	nowInLoc := time.Now().In(cfg.Location())
	// 8. Render buffer
	buf := RenderWeekBuffer(*week, areaNames, woy.Year, woy.WeekIndex(), weekStart,
		nowInLoc)
	// 9. Create temp file, write buf
	pattern := fmt.Sprintf("focustime-%dw%d_*", woy.Year, woy.Week)
	tmp, err := os.CreateTemp("", pattern)
	if err != nil {
		return err
	}
	tempPath := tmp.Name()
	defer os.Remove(tempPath)
	if _, err := tmp.WriteString(buf); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	// 10. Launch editor
	if err := LaunchEditor(tempPath); err != nil {
		return fmt.Errorf("editor exited with error: %w", err)
	}
	// 11. Parse file
	fileBz, err := os.ReadFile(tempPath)
	if err != nil {
		return err
	}
	parsed, err := ParseWeekBuffer(fileBz, areaNames, week.Areas)
	if err != nil {
		return err
	}
	// 12. Update yf.Weeks, SaveYearFile
	yf.Weeks[woy.WeekIndex()] = &parsed
	return SaveYearFile(cfg, yf)
}

// weekStartFor returns the Monday of the given ISO week in UTC.
// Uses the rule that Jan 4 is always in week 1.
func weekStartFor(year, week int) time.Time {
	// Jan 4 is always in week 1
	jan4 := time.Date(year, 1, 4, 0, 0, 0, 0, time.UTC)
	weekday := int(jan4.Weekday())
	if weekday == 0 {
		weekday = 7 // Sunday = 7
	}
	// Monday of week 1
	monday1 := jan4.AddDate(0, 0, 1-weekday)
	return monday1.AddDate(0, 0, 7*(week-1))
}
