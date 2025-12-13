package algos

import "fmt"

// You are given a 2D grid representing a maze. Each cell is either open (0) or
// blocked (1). You are also given a start position and an end position.
//
// Write a function that returns True if there exists a path from the start to the
// end position using only up, down, left, or right moves without crossing any
// blocked cells.
//
// Example
// Input
// ```
// maze = [
//     [0, 0, 1],
//     [1, 0, 1],
//     [1, 0, 0]
// ]
// start = (0, 0)
// end = (2, 2)
// ```
//
// Output:
// ```
// True
// ```
//
// Constraints:
// - `1 <= maze.length`, `maze[0].length <= 100`
// - `maze[r][c]` is either 0 (open) or 1 (blocked)
// - Start and end are valid open cells

func Sol4(grid [][]int, start, end [2]int) (hasPath bool) {

	maxR := len(grid) - 1
	if maxR < 0 {
		return false
	}
	maxC := len(grid[0]) - 1
	if maxC < 0 {
		return false
	}

	// start in grid?
	// end in grid?
	// start, end blocked?

	inBounds := func(pt [2]int) bool {
		r, c := pt[0], pt[1]
		if r < 0 || r > maxR ||
			c < 0 || c > maxC {
			return false
		}
		return true
	}

	if !inBounds(start) || !inBounds(end) {
		return false
	}

	startVal := grid[start[0]][start[1]]
	endVal := grid[end[0]][end[1]]
	if startVal == 1 || endVal == 1 {
		return false
	}

	seen := make(map[[2]int]struct{})
	pathsToExplore := [][2]int{
		start,
	}
	for len(pathsToExplore) > 0 {
		path := pathsToExplore[len(pathsToExplore)-1]
		pathsToExplore = pathsToExplore[:len(pathsToExplore)-1]

		if _, isSeen := seen[path]; isSeen {
			continue
		}

		if !inBounds(path) {
			continue
		}

		r, c := path[0], path[1]
		if grid[r][c] == 1 {
			continue
		}

		seen[path] = struct{}{}
		if path == end {
			return true
		}

		pathsToExplore = append(pathsToExplore,
			[2]int{r + 1, c},
			[2]int{r - 1, c},
			[2]int{r, c + 1},
			[2]int{r, c - 1},
		)
	}

	return false
}

func (s *S) TestSol4() {

	type TC struct {
		name       string
		grid       [][]int
		start, end [2]int
		want       bool
	}
	for tcIdx, tc := range []TC{
		{
			name: "simple path exists",
			grid: [][]int{
				{0, 1, 0},
				{0, 0, 1},
				{1, 0, 0},
			},
			start: [2]int{0, 0},
			end:   [2]int{2, 2},
			want:  true,
		},
		{
			name: "no path because of wall near end",
			grid: [][]int{
				{0, 1, 0},
				{0, 0, 1},
				{1, 0, 1},
			},
			start: [2]int{0, 0},
			end:   [2]int{2, 2},
			want:  false,
		},
		{
			name: "single cell open start equals end",
			grid: [][]int{
				{0},
			},
			start: [2]int{0, 0},
			end:   [2]int{0, 0},
			want:  true,
		},
		{
			name: "single cell blocked",
			grid: [][]int{
				{1},
			},
			start: [2]int{0, 0},
			end:   [2]int{0, 0},
			want:  false,
		},
		{
			name: "start out of bounds",
			grid: [][]int{
				{0, 0},
				{0, 0},
			},
			start: [2]int{-1, 0},
			end:   [2]int{1, 1},
			want:  false,
		},
		{
			name: "end out of bounds",
			grid: [][]int{
				{0, 0},
				{0, 0},
			},
			start: [2]int{0, 0},
			end:   [2]int{2, 0},
			want:  false,
		},
		{
			name: "start blocked",
			grid: [][]int{
				{1, 0},
				{0, 0},
			},
			start: [2]int{0, 0},
			end:   [2]int{1, 1},
			want:  false,
		},
		{
			name: "end blocked",
			grid: [][]int{
				{0, 0},
				{0, 1},
			},
			start: [2]int{0, 0},
			end:   [2]int{1, 1},
			want:  false,
		},
		{
			name: "larger open grid path exists",
			grid: [][]int{
				{0, 0, 0, 0},
				{0, 0, 0, 0},
				{0, 0, 0, 0},
				{0, 0, 0, 0},
			},
			start: [2]int{0, 0},
			end:   [2]int{3, 3},
			want:  true,
		},
		{
			name: "wall cuts grid in half no path",
			grid: [][]int{
				{0, 0, 0, 0},
				{1, 1, 1, 1},
				{0, 0, 0, 0},
				{0, 0, 0, 0},
			},
			start: [2]int{0, 0},
			end:   [2]int{3, 3},
			want:  false,
		},
		{
			name: "must go around obstacles path exists",
			grid: [][]int{
				{0, 0, 0, 0},
				{1, 1, 1, 0},
				{0, 0, 0, 0},
				{0, 1, 1, 0},
			},
			start: [2]int{0, 0},
			end:   [2]int{3, 3},
			want:  true,
		},
	} {
		s.Run(fmt.Sprintf("tc %d %s", tcIdx, tc.name), func() {
			got := Sol4(tc.grid, tc.start, tc.end)
			s.Require().Equal(tc.want, got)
		})
	}

}
