package src

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

type StartCfg struct {
	HomeDir string
}

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
	xdgDataHome := XdgDataHome(cfg)
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

	fp := filepath.Join(DirFTData(cfg), "areas.json")
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

// CreateFocusTimeDir: Creates the primary data directory if it does not already
// exist. Otherwise, does nothing.
func CreateFocusTimeDir(cfg StartCfg) error {
	ftDir := DirFTData(cfg)
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
func XdgConfigHome(cfg StartCfg) string {
	if v := os.Getenv("XDG_CONFIG_HOME"); v != "" {
		return v
	}
	home := cfg.HomeDir
	return filepath.Join(home, ".config")
}

// XdgDataHome returns the XDG data directory, falling back to
// $HOME/.local/share when XDG_DATA_HOME is not set.
// Data holds persistent downloaded assets.
func XdgDataHome(cfg StartCfg) string {
	if v := os.Getenv("XDG_DATA_HOME"); v != "" {
		return v
	}
	home := cfg.HomeDir
	return filepath.Join(home, ".local", "share")
}

// XdgStateHome returns the XDG state directory, falling back to
// $HOME/.local/state when XDG_STATE_HOME is not set.
// State in the XDG spec is runtime info that persists but is not user-editable.
// It typically holds session state like logs and runtime history.
func XdgStateHome(cfg StartCfg) string {
	if v := os.Getenv("XDG_STATE_HOME"); v != "" {
		return v
	}
	home := cfg.HomeDir
	return filepath.Join(home, ".local", "state")
}

// XdgCacheHome returns the XDG cache directory, falling back to $HOME/.cache
// when XDG_CACHE_HOME is not set. Cache is used for ephemeral speed-ups. Cache
// is meant to be safe to delete at all times.
func XdgCacheHome(cfg StartCfg) string {
	if v := os.Getenv("XDG_CACHE_HOME"); v != "" {
		return v
	}
	home := cfg.HomeDir
	return filepath.Join(home, ".cache")
}

// DirFTData returns the "$XDG_DATA_HOME/focustime" directory.
func DirFTData(cfg StartCfg) string {
	return filepath.Join(XdgDataHome(cfg), "focustime")
}

// WoY represents an ISO week-of-year, identified by its year and week number.
type WoY struct {
	Year int
	Week int
}

// TimeToWoY converts t to its ISO week-of-year in UTC and returns the
// corresponding WoY value.
func TimeToWoY(t time.Time) WoY {
	t = t.UTC()
	year, weekOfYear := t.ISOWeek()
	return WoY{
		Year: year,
		Week: weekOfYear,
	}
}
