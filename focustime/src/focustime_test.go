package src

import (
	"errors"
	"fmt"
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

// TestGetAreasOfWeekForTime_FindsLatestChangeInSameYear
// ensures we pick the latest StartFrom within the same year whose week
// is <= the target week.
func (s *S) TestGetAreasOfWeekForTime_FindsLatestChangeInSameYear() {
	changes := []AreasChange{
		{
			StartFrom: WoY{Year: 2025, Week: 1},
			Areas:     []string{"a", "b"},
		},
		{
			StartFrom: WoY{Year: 2025, Week: 10},
			Areas:     []string{"c", "d"},
		},
		{
			StartFrom: WoY{Year: 2025, Week: 20},
			Areas:     []string{"e", "f"},
		},
	}

	// Pick a date that falls in ISO week 15 of 2025.
	// We don't care about the exact calendar mapping in this test; we just
	// want a week that is >= 10 and < 20 so the second change wins.
	t := time.Date(2025, 4, 7, 0, 0, 0, 0, time.UTC) // somewhere in spring 2025

	got, err := GetAreasOfWeekForTime(changes, t)
	s.Require().NoError(err)

	s.Equal(WoY{Year: 2025, Week: 10}, got.StartFrom)
	s.Equal([]string{"c", "d"}, got.Areas)
}

// TestGetAreasOfWeekForTime_IgnoresOtherYears ensures entries from other
// years are ignored even if their weeks would otherwise match.
func (s *S) TestGetAreasOfWeekForTime_IgnoresOtherYears() {
	changes := []AreasChange{
		{
			StartFrom: WoY{Year: 2024, Week: 1},
			Areas:     []string{"old"},
		},
		{
			StartFrom: WoY{Year: 2025, Week: 5},
			Areas:     []string{"new"},
		},
	}

	t := time.Date(2025, 2, 1, 0, 0, 0, 0, time.UTC)

	got, err := GetAreasOfWeekForTime(changes, t)
	s.Require().NoError(err)

	s.Equal(WoY{Year: 2025, Week: 5}, got.StartFrom)
	s.Equal([]string{"new"}, got.Areas)
}

// TestGetAreasOfWeekForTime_NoAreasForYear ensures we get the sentinel
// ErrAreasMissingForYear when no change entry exists for the target year.
func (s *S) TestGetAreasOfWeekForTime_NoAreasForYear() {
	changes := []AreasChange{
		{
			StartFrom: WoY{Year: 2023, Week: 10},
			Areas:     []string{"legacy"},
		},
		{
			StartFrom: WoY{Year: 2024, Week: 10},
			Areas:     []string{"still-legacy"},
		},
	}

	t := time.Date(2025, 3, 1, 0, 0, 0, 0, time.UTC)

	got, err := GetAreasOfWeekForTime(changes, t)
	s.Nil(got, "expected nil areas when year data is missing")
	s.Require().Error(err)
	s.True(
		errors.Is(err, ErrAreasMissingForYear),
		"fmt error should wrap ErrAreasMissingForYear, got: %v",
		err,
	)
}
