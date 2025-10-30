# Depth-First Search (DFS)

- [Q: Is this bottom statement syntactically correct? If not, describe why and how to](#q-is-this-bottom-statement-syntactically-correct-if-not-describe-why-and-how-to)
- [Ex 0: Fill Image Pixels](#ex-0-fill-image-pixels)
- [Ex 1: Graph Traversal without Grid](#ex-1-graph-traversal-without-grid)
- [Ex 2: Number of Islands](#ex-2-number-of-islands)
- [Ex 3: Path Exists in Maze](#ex-3-path-exists-in-maze)
- [Ex 4: Count Connected Components in an Undirected Graph](#ex-4-count-connected-components-in-an-undirected-graph)
- [Ex 5: Image Fill in a 2D Grid](#ex-5-image-fill-in-a-2d-grid)
- [25-11-04](#25-11-04)


## Q: Is this bottom statement syntactically correct? If not, describe why and how to
fix it.

```python
image: List[List[int]]
max_r: int

max_c = 0 if max_r === 0 else len(image[0])
```

A: It's wrong. The equals operator in Python is "==", not "===" like in TS.

from typing import List, Set, Tuple

def foo(x: List[any]):
    """Docs on foo"""
    return x + 1

foo(1)

x: Set = set([1, 2, 3])

## Ex 0: Fill Image Pixels

Source Info:
- This is [Leetcode 733: Flood Fill](https://leetcode.com/problems/flood-fill/description/)
- Full Impls: Reference code for questions. `dfs_00_test.go`, `dfs_00_test.py`

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

Use depth first search starting out from the starting pixel using coordinates to
overwrite values in the image.

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
Could it instead be the following and mutate `img`? Why?
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

Q (Cloze): A function takes a slice as an argument. In Go, a
slice header (the value passed around) contains a pointer to an underlying array,
a length, and a capacity. 

Q (cont.): Passing a slice to a function passes a copy of the
header into that func, but header of the original value and the copy in the
function both point to the same underlying array.

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

## Ex 1: Graph Traversal without Grid

You are given a directed graph represented as an adjacency list. Each key in the graph is a unique node identifier (a string), and the corresponding value is a list of nodes that it directly connects to (its neighbors).

Q: Implement a function that performs a depth-first search (DFS) starting from a given node. Your function should return a list of all nodes reachable from the starting node, in the order they were first visited.

Example: 

Input
```
graph = {
    'A': ['B', 'C'],
    'B': ['D'],
    'C': ['E'],
    'D': [],
    'E': ['F'],
    'F': []
}
start = 'A'
```

Output:
```
['A', 'B', 'D', 'C', 'E', 'F']
```

Constraints:
- All node names are unique strings.
- The graph may contain cycles (but no self-loops).
- You may assume the input graph is valid and all referenced nodes are included.
- You may not revisit nodes 



Q: Why is important to communicate that you "may not revisit nodes"?

This implies we'll need to use a visited set to avoid infinite loops.

Q: Is this problem solvable with both DFS and BFS? If not, why? And if so, when
should you use one versus the other?

Both DFS and BFS are viable. 

BFS is better if you want to know the shortest path
to each reachable node. DFS doesn't guarantee shortest paths.

DFS is better for deep or narrow structures. BFS uses a queue and tends to
consume more memory in wide graphs.


Q: We often say that DFS is implemented with stack and BFS is implemented with a
queue.

How is that the case when using recursion with DFS?

Recursion implements an implicit stack, the call stack. You can also write DFS iteratively with
an explicit stack.

Q: The queue behavior order acronym is "FIFO" (first-in-first-out).

Q: The stack behavior order acronym is "LIFO" (last-in-first-out).

Q: Implement a queue of strings in Python.

```python
from collections import deque

start: str # pretend we have it
queue: deque[str] = deque([start])
```

Q: Queues in Python are imported from what package? 

```
collections.deque
```

Q: What does "deque" mean in Python?

Double-ended queue.

Q: Why use a `deque` instead of a `List` in BFS?

A `deque` (double-ended queue) has O(1) time complexity for popping from teh
left (`deque.popleft`). Whereas, `list.pop(0)` would be O(n) because it shifts
all elements.

Q: In Python, how do you add to a `deque`?

`deque.append`

## Ex 2: Number of Islands

You are given a 2D grid of '1's (land) and '0's (water). An island is formed by connecting adjacent lands horizontally or vertically. You may assume all four edges of the grid are surrounded by water.

Q: Write a function to count the number of distinct islands in the grid.

Example: 

Input
```
grid = [
    [1,1,0,0,0],
    [1,1,0,0,0],
    [0,0,1,0,0],
    [0,0,0,1,1]
]
```

Output: 3








## Ex 3: Path Exists in Maze

You are given a 2D grid representing a maze. Each cell is either open (0) or
blocked (1). You are also given a start position and an end position.

Write a function that returns True if there exists a path from the start to the
end position using only up, down, left, or right moves without crossing any
blocked cells.

Example 
Input
```
maze = [
    [0, 0, 1],
    [1, 0, 1],
    [1, 0, 0]
]
start = (0, 0)
end = (2, 2)
```

Output: 
```
True
```

Constraints:
- `1 <= maze.length`, `maze[0].length <= 100`
- `maze[r][c]` is either 0 (open) or 1 (blocked)
- Start and end are valid open cells

```python
def path_exists(maze: list[list[int]], start: tuple[int, int], end: tuple[int, int]) -> bool:
    ...
```

Q: Describe tha algorithm solution in words?

Check if start is blocked? If so, return False.
Check if end is blocked. If so, return False.
Beginning from the start, 
DFS(up, left, down, right)



## Ex 4: Count Connected Components in an Undirected Graph

You are given an undirected graph represented as an adjacency list. Each key in the graph is a node (string), and the value is a list of nodes that it is directly connected to.

Write a function that returns the number of connected components in the graph. Two nodes are in the same component if there is a path between them, directly or indirectly.

Example Input
```
graph = {
    "A": ["B"],
    "B": ["A"],
    "C": [],
    "D": ["E"],
    "E": ["D"]
}
```

Output: 3

Constraints:
- All node names are unique strings.
- The graph may be disconnected.
- It is undirected: if A connects to B, then B connects to A.

## Ex 5: Image Fill in a 2D Grid

You are given a 2D array image of integers representing pixel values in a bitmap. Each pixel is identified by its row and column index.

Write a function that changes the color of a given starting pixel and all 4-directionally connected pixels that have the same original color to a new color.

Example Input
```
image = [
    [1, 1, 1],
    [1, 1, 0],
    [1, 0, 1]
]
sr = 1
sc = 1
new_color = 2
```

Output:
```
[
    [2, 2, 2],
    [2, 2, 0],
    [2, 0, 1]
]
```

## 25-11-04

Q: Why do we call the Python `dict` a hash map? Where's the term hash coming
from?

A hash map is a general data structure in many languages that uses a hash
function to map keys to indices in a table for fast access. Accessing a value
from some hash of a key is constant time on average. 

Map is the mathematical term from mapping one element from one set uniquely to
another.
