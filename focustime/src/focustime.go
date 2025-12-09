package src

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

type AreasChange struct {
	StartFrom WoY
	Areas     []string
}

var (
	ErrAreasMissingForYear = errors.New("focustime: areas missing for requested year")
)

func GetAreasOfWeekForTime(
	changes []AreasChange,
	t time.Time,
) (areas *AreasChange, err error) {
	woy := TimeToWoY(t)

	latestAreasIdx := -1
	for changeIdx, change := range changes {
		if change.StartFrom.Year != woy.Year {
			continue
		}
		if change.StartFrom.Week <= woy.Week {
			latestAreasIdx = changeIdx
		}
	}

	if latestAreasIdx == -1 {
		return nil, fmt.Errorf("%w: { year: %d, woy: %d }", ErrAreasMissingForYear, woy.Year, woy.Week)
	}

	return &changes[latestAreasIdx], nil
}

func LoadAreasFile() ([]AreasChange, error) {
	var changes []AreasChange
	xdgDataHome := XdgDataHome()
	fileInfo, err := os.Stat(xdgDataHome)
	if err != nil && os.IsNotExist(err) {
		err := os.MkdirAll(xdgDataHome, 0o644)
		return changes, err
	} else if err != nil {
		return changes, err
	}

	if !fileInfo.IsDir() {
		return changes, fmt.Errorf("file exists in place of expected directory at %s", xdgDataHome)
	}

	err = CreateFocuseTimeDir()
	if err != nil {
		return changes, err
	}

	fp := filepath.Join("focustime", "areas.json")
	fileInfo, err = os.Stat(fp)
	switch {
	case err != nil && os.IsNotExist(err):

		jsonBz, _ := json.MarshalIndent(changes, "", "  ")
		err = os.WriteFile(fp, jsonBz, 0o644)
		return changes, err
	case err != nil:
		return changes, err
	}

	fileBz, err := os.ReadFile(fp)
	if err != nil {
		return changes, err
	}

	err = json.Unmarshal(fileBz, &changes)
	if err != nil {
		return changes, err
	}

	return changes, nil
}

// CreateFocuseTimeDir: Creates the primary data directory if it does not already
// exist. Otherwise, does nothing.
func CreateFocuseTimeDir() error {
	xdgDataHome := XdgDataHome()
	ftDir := filepath.Join(xdgDataHome, "focustime")
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

	err = os.MkdirAll(ftDir, 0o644)
	return err
}

func XdgConfigHome() string {
	if v := os.Getenv("XDG_CONFIG_HOME"); v != "" {
		return v
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config")
}

func XdgDataHome() string {
	if v := os.Getenv("XDG_DATA_HOME"); v != "" {
		return v
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".local", "share")
}

func XdgStateHome() string {
	if v := os.Getenv("XDG_STATE_HOME"); v != "" {
		return v
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".local", "state")
}

func XdgCacheHome() string {
	if v := os.Getenv("XDG_CACHE_HOME"); v != "" {
		return v
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".cache")
}

type WoY struct {
	Year int
	Week int
}

// TODO: fn that converts a time into a week of the year.
func TimeToWoY(t time.Time) WoY {
	t = t.UTC()
	year, weekOfYear := t.ISOWeek()
	return WoY{
		Year: year,
		Week: weekOfYear,
	}
}
