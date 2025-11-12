package algos

// You're given an image represented as a 2D grid of integers. Each cell contains a
// color value. You're also given a starting pixel `(sr, sc)` and a `newColor`. The
// task is to recolor the entire connected region (4-directionally connected) that
// shares the original color of the starting pixel with the new color.
//
// Example:
// ```
// image = [
//   [1,1,1],
//   [1,1,0],
//   [1,0,1]
// ]
// sr, sc = 1, 1
// newColor = 2
// ```
// This changes all 1s connected to (1, 1) into 2s.

import (
	"testing"

	"github.com/stretchr/testify/suite"
)

type Point [2]int

func Sol0_RecursiveDFS(img [][]int, newColor int, startPixel Point) [][]int {
	var (
		maxRow          int // max valid row. Ex: 3 for a 4x6 matrix
		maxCol          int // max valid col. Ex: 5 for a 4x6 matrix
		startPixelColor int // color of startPixel before modification
	)

	maxRow = len(img) - 1
	if maxRow < 0 {
		return img
	}
	maxCol = len(img[0]) - 1 // Img is rectangular, so each row is the same length.
	if maxCol < 0 {
		return img
	}

	r, c := startPixel[0], startPixel[1]
	if (r < 0 || r > maxRow) || // stop if the img is empty
		(c < 0 || c > maxCol) { // outside img bounds
		return img
	}

	startPixelColor = img[r][c]
	if startPixelColor == newColor {
		return img // the job is done already
	}

	seenPixels := make(PointSet)

	// DFS from startPixel, re-coloring parts of the Image
	// 4 directions
	var dfsFillFromPoint func(pixel Point)
	dfsFillFromPoint = func(pixel Point) {
		_, seen := seenPixels[pixel]
		if seen {
			return
		}

		r, c := pixel[0], pixel[1]
		if (c > maxCol || c < 0) ||
			(r > maxRow || r < 0) ||
			img[r][c] != startPixelColor {
			return // skip invalid pixels
		}

		seenPixels[pixel] = struct{}{}

		if img[r][c] == newColor {
			return
		}

		img[r][c] = newColor

		dfsFillFromPoint(Point{r + 1, c}) // Down
		dfsFillFromPoint(Point{r, c - 1}) // Left
		dfsFillFromPoint(Point{r - 1, c}) // Up
		dfsFillFromPoint(Point{r, c + 1}) // Right
	}

	dfsFillFromPoint(startPixel)

	return img
}

type PointSet = map[Point]struct{}

type S struct {
	suite.Suite
}

func Test(t *testing.T) {
	suite.Run(t, new(S))
}

func (s *S) TestFillImg() {
	type tc struct {
		name     string
		imgIn    [][]int
		start    Point
		newColor int
		want     [][]int
	}
	tests := []tc{
		{
			name: "Small connected region of 1s",
			imgIn: [][]int{
				{1, 1, 1},
				{1, 1, 0},
				{1, 0, 1},
			},
			start:    Point{0, 0},
			newColor: 2,
			want: [][]int{
				{2, 2, 2},
				{2, 2, 0},
				{2, 0, 1},
			},
		},
		{
			name: "Fill follows a winding 1-path",
			imgIn: [][]int{
				{1, 0, 0, 0},
				{1, 1, 0, 0},
				{0, 1, 1, 1},
				{0, 0, 0, 1},
			},
			start:    Point{0, 0},
			newColor: 2,
			want: [][]int{
				{2, 0, 0, 0},
				{2, 2, 0, 0},
				{0, 2, 2, 2},
				{0, 0, 0, 2},
			},
		},
		{
			name: "0 is surrounded by 1s; fill from the center",
			imgIn: [][]int{
				{1, 1, 1},
				{1, 0, 1},
				{1, 1, 1},
			},
			start:    Point{0, 0},
			newColor: 2,
			want: [][]int{
				{2, 2, 2},
				{2, 0, 2},
				{2, 2, 2},
			},
		},
		{
			name: "Diagonals are not connected in 4-directional DFS",
			imgIn: [][]int{
				{1, 0, 0},
				{0, 1, 0},
				{0, 0, 1},
			},
			start:    Point{0, 0},
			newColor: 2,
			want: [][]int{
				{2, 0, 0},
				{0, 1, 0},
				{0, 0, 1},
			},
		},
		{
			name:     "Empty image",
			imgIn:    [][]int{},
			start:    Point{0, 0},
			newColor: 2,
			want:     [][]int{},
		},
		{
			name: "Already filled",
			imgIn: [][]int{
				{2, 2, 2},
				{2, 2, 0},
				{2, 0, 1},
			},
			start:    Point{0, 0},
			newColor: 2,
			want: [][]int{
				{2, 2, 2},
				{2, 2, 0},
				{2, 0, 1},
			},
		},
	}

	for _, tc := range tests {
		s.Run(tc.name, func() {
			got := Sol0_RecursiveDFS(tc.imgIn, tc.newColor, tc.start)
			s.Require().Equal(tc.want, got)
			got = Sol0_IterDFS(tc.imgIn, tc.newColor, tc.start)
			s.Require().Equal(tc.want, got)
		})
	}
}

func Sol0_IterDFS(img [][]int, newColor int, startPixel Point) [][]int {
	var (
		maxRow          int // max valid row. Ex: 3 for a 4x6 matrix
		maxCol          int // max valid col. Ex: 5 for a 4x6 matrix
		startPixelColor int // color of startPixel before modification
	)

	maxRow = len(img) - 1
	if maxRow < 0 { //  no rows implies no columns
		return img
	}
	maxCol = len(img[0]) - 1
	if maxCol < 0 {
		return img // N x N matrix only makes sense 1 x 1 or larger
	}

	// startPixel in grid?
	r, c := startPixel[0], startPixel[1]
	if (r < 0 || r > maxRow) || (c < 0 || c > maxCol) {
		return img
	}

	// newColor already set on the pixel?
	startPixelColor = img[r][c]
	if startPixelColor == newColor {
		return img
	}

	// iterative DFS is to create a stack with the start
	// loop over the stack,
	// storing pixels "seen"?
	// and add neighbors to the stack in each iteration
	// this will DFS process all neighbors without recursion
	pixelStack := []Point{startPixel}
	pixelsSeen := make(map[Point]struct{})
	for len(pixelStack) > 0 {
		popped := pixelStack[len(pixelStack)-1]
		pixelStack = pixelStack[:len(pixelStack)-1]

		_, seen := pixelsSeen[popped]
		if seen {
			continue
		}
		pixelsSeen[popped] = struct{}{}

		r, c := popped[0], popped[1]
		if (r < 0 || r > maxRow) ||
			(c < 0 || c > maxCol) ||
			(img[r][c] == newColor) ||
			(img[r][c] != startPixelColor) {
			continue
		}

		img[r][c] = newColor
		pixelStack = append(pixelStack, []Point{
			{r - 1, c}, // up
			{r, c + 1}, // right
			{r + 1, c}, // down
			{r, c - 1}, // left
		}...)
	}

	return img
}
