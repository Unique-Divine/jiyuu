# Unique-Divine/algos/algos-notes

Records questions, answers, examples, and extracted facts in a format where each 
concept can be deliberately practiced and reinforced with spaced repetition
software like Anki.

## Contents

- [Contents](#contents)
- [Ex 0: Fill Image Pixels](#ex-0-fill-image-pixels)

### Ex 0: Fill Image Pixels

Source Info:
- This is [Leetcode 733: Flood Fill](https://leetcode.com/problems/flood-fill/description/)
- Full Impls: Reference code for questions. `000_dfs_test.go`

問題文:

You're given an image represented as a 2D grid of integers. Each cell contains a
color value. You're also given a starting pixel `(sr, sc)` and a `newColor`. The
task is to recolor the entire connected region (4-directionally connected) that
shares the original color of the starting pixel with the new color.

Example:
```
image = [
  [1,1,1],
  [1,1,0],
  [1,0,1]
]
sr, sc = 1, 1
newColor = 2
```
This changes all 1s connected to (1, 1) into 2s.

Q: (Cloze) In one sentence, describe how you solve this problem.

Use depth first search starting out from the starting pixel using coordinates to overwrite values in the image.

Q: Write out the function signature for the problem in Python.
```python
def fill_img(
    image: List[List[int]],
    new_color: int,
    start_pixel: Tuple[int, int],
) -> List[List[int]]:
    """Fills the start pixel and all neighbors of the same color with the new
    color.
    """
```

Q: Write out the function signature for the problem in Go.
```go
func Sol0_RecursiveDFS(img [][]int, newColor int, startPixel Point) [][]int {
     // or Sol0_IterDFS
}
```


Q: Suppose this is the function signature in Go.
```go
func Sol0_RecursiveDFS(img [][]int, newColor int, startPixel Point) [][]int {}
```
Could it instead be the following and mutate `img` inplace? Why?
```go
func Sol0_RecursiveDFS(img [][]int, newColor int, startPixel Point) {}
```

A: Yes, mutating elements of a slice does not change the slice header. In Go, a
slice header (the value passed around) contains a pointer to an underlying array,
a length, and a capacity. Passing a slice to a function passes a copy of the
header into that func, but header of the original value and the copy in the
function both point to the same underlying array.

Thus, if you mutate elements with `img[r][c] = x`, the caller sees those changes.
If you plan on appending to the slice or re-slicing a new one using any sort of
slice operator, then you'll want to return the new slice rather than counting on
mutation.

Q (Cloze): "A function takes a slice as an argument."
In Go, a slice, or slice header, is defined by these fields:
- `array` - pointer to some underlying array. More precisely, this pointer is the exact
memory address where the first element of the slice lives.
- `len` - length of the slice
- `cap` - capacity or size of the slice

A slice does not contain elements. Rather, it describes where elements are and
how many to use.

In intuitive terms, we can think of a slice as a window of some underlying array.

The majority of the Go standard library and runtime is written in Go itself.
```go
// from src/runtime/slice.go, the "runtime" package
type slice struct {
	array unsafe.Pointer
	len   int
	cap   int
}
```
The runtime also includes a small amount of assembly, a minimal bit of C, and
some compiler internals written in Go. The compiler, standard library, GC,
scheduler, and runtime are all implemted almost entirely in Go.

Q: Since slice and slice header mean the same thing, why do we use the term slice
header? Where's the word "header" coming from?

In systems programming, a header is a small, fixed-size structure containing
metadata that tells the runtime how to interpret larger, variable-size data
behind it.

You can see from the `slice` definition that all slices are 3 machine words. The
pointer, length, and capacity are all fixed size, while the actual elements all
live in the array.
```go
type slice struct {
	array unsafe.Pointer
	len   int
	cap   int
}
```

Q (cont.): Passing a slice to a function passes a copy of the
header into that func. The header of the original value and the copy that lives
in the function both point to the same underlying array.

Q: What's the key, common element between recursive DFS and iterative DFS?

A: Both approaches use a stack. It's just that recursive DFS uses the call stack
managed by the runtime, whereas iterative DFS explicitly stores a stack using a
data structure like a slice or list.

Q: Describe all of the stop conditions that occur before the DFS loop?

A:
- Grid must be well-posed, rectangular and non-empty
- Start pixel given must be within the grid
- New color should actually be new. If it already matches, there's no reason to
traverse and change anything.

Q: Using a recursive DFS approach, how do you write a locally scoped function that can call itself? There's a trick to make the compiler happy.

A: You have to declare the variable for the fn with `var` first. If you try to
use  `fn := func() {}`, the scope inside of the function won't be able to access
the function.  The following would work:
```go
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
```

Q: Code up the recursive DFS function, assuming you validated that the
`startPixel` is in the `img`, the `img` is not empty, and you have defined the
following variables already.
```go
var (
    maxRow          int // max valid row. Ex: 3 for a 4x6 matrix
    maxCol          int // max valid col. Ex: 5 for a 4x6 matrix
    startPixelColor int // color of startPixel before modification
)
```

A:
```go
type PointSet = map[Point]struct{}
type Point [2]int

func Sol0_RecursiveDFS(img [][]int, newColor int, startPixel Point) [][]int {
    var (
        maxRow          int // max valid row. Ex: 3 for a 4x6 matrix
        maxCol          int // max valid col. Ex: 5 for a 4x6 matrix
        startPixelColor int // color of startPixel before modification
    )

    // ... validation and early returns implemented already

    seenPixels := make(PointSet)
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
}
```

Q: Code up the iterative DFS approach, assuming you validated that the
`startPixel` is in the `img`, the `img` is not empty, and you have defined the
following variables already.
```go
var (
    maxRow          int // max valid row. Ex: 3 for a 4x6 matrix
    maxCol          int // max valid col. Ex: 5 for a 4x6 matrix
    startPixelColor int // color of startPixel before modification
)
```

A:
```go
type PointSet = map[Point]struct{}
type Point [2]int

func Sol0_IterDFS(img [][]int, newColor int, startPixel Point) [][]int {
    var (
        maxRow          int // max valid row. Ex: 3 for a 4x6 matrix
        maxCol          int // max valid col. Ex: 5 for a 4x6 matrix
        startPixelColor int // color of startPixel before modification
    )

    // ... validation and early returns implemented already

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
```

Q: Write out the early returns and definitions for `maxRow`, `maxCol`,
and `startPixelColor`.

A:
```go
type PointSet = map[Point]struct{}
type Point [2]int

func Sol0_RecursiveDFS(img [][]int, newColor int, startPixel Point) [][]int {
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
}
```

ANKI LINE ------------------------------------------------------------

