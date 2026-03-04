# Unique-Divine/algos/algos-notes

Records questions, answers, examples, and extracted facts in a format where each 
concept can be deliberately practiced and reinforced with spaced repetition
software like Anki.

## Contents

- [Contents](#contents)
- [Ex 0: Fill Image Pixels](#ex-0-fill-image-pixels)
- [Contents](#contents)
  - [Ex 0: Fill Image Pixels](#ex-0-fill-image-pixels)
- [ANKI LINE ------------------------](#anki-line-------------------------)
- [E1: Enqueue and Dequeue (Basic Ops in Go)](#e1-enqueue-and-dequeue-basic-ops-in-go)
- [E2: O(1) vs O(n) Trap (Bad Queue Implementation in Go)](#e2-o1-vs-on-trap-bad-queue-implementation-in-go)
- [005: Breadth First Search (BFS) Intro](#005-breadth-first-search-bfs-intro)
  - [Full BFS Implementation in Go](#full-bfs-implementation-in-go)
  - [Go Equivalents of Python Deque Ops (for Anki-style recall)](#go-equivalents-of-python-deque-ops-for-anki-style-recall)

### Ex 0: Fill Image Pixels

Source Info:
- This is [Leetcode 733: Flood Fill](https://leetcode.com/problems/flood-fill/description/)
- Full Impls: Reference code for questions. `000_dfs_test.go`

Practice Problem:

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

## E1: Enqueue and Dequeue (Basic Ops in Go)

Practice Problem: 

Implement a simple queue `struct` in Go using a slice. Your queue must support
`Enqueue` and `Dequeue` methods. 

```go
// Fill in the methods.
type Queue[T any] struct {
    data []T
}

func (q *Queue[T]) Enqueue(x T) {
    // TODO
}

func (q *Queue[T]) Dequeue() (elem T, ok bool) {
    // TODO
}
```

A:

```go
type Queue[T any] struct {
    data []T
}

// Enqueue is the "[en]ter [queue]" operation (mnemonic).
// Amortized O(1): append to end of slice.
func (q *Queue[T]) Enqueue(x T) {
    q.data = append(q.data, x)
}

func (q *Queue[T]) Dequeue() (elem T, ok bool) {
    var zero T
    if len(q.data) == 0 {
        return zero, false
    }

    // Oldest element is at the front.
    x := q.data[0]
     
    q.data[0] = zero // Optional GC hygiene if T is a reference type:

    // Move the slice window forward. O(1).
    q.data = q.data[1:]

    return x, true
}
```

Q: What is the amortized time complexity of `Enqueue` in this implementation?

A:
Amortized O(1), because `append` is amortized O(1) for slices.

Q: What is the time complexity of `Dequeue` in this implementation (ignoring GC details)?

A:
O(1), because re-slicing (`q.data = q.data[1:]`) only adjusts the slice header.

Q: In Python, `list.pop(0)` is O(n) because elements are shifted. 
What is the complexity of `q = q[1:]` if `q` is some slice, and why?

A:
A Go slice is a small header (`pointer`, `len`, `cap`) pointing at an underlying array. Re-slicing changes the header's pointer and length; it does not move elements in memory.

Q: What subtle issue can happen if we only do `q.data = q.data[1:]` when `T` is a reference type (e.g., `*MyStruct`, `[]byte`)?

A: 
The underlying array still holds references to old elements, so they may not
be garbage collected. The slice no longer "sees" them, but the GC does until the
array itself is released.

Q: How can you make dequeued elements eligible for GC sooner?

A:
Before re-slicing, overwrite the element with the zero value:

```go
var q []T
var defVal T
x := q[0]        // if len(q) > 0
q = q[1:]
q[0] = defVal
```

When you slice a queue forward with:

```go
q = q[1:]
```

the **backing array** is still on the heap. Its slot at index 0 still contains
whatever pointer value was stored there. Even though the slice length changed,
the GC still sees the backing array and all pointer-bearing elements inside it.
If `T` is a reference type, that stale pointer keeps the old object alive.

If instead you overwrite the slot:

```go
var zero T
q[0] = zero
q = q[1:]
```

you remove that pointer from memory. For reference types, the zero value is `nil`
(or a struct whose pointer fields are nil), so the GC stops seeing the old object
through this element. If no other references point to it, it becomes collectible.

Examples:

**If `T` is a pointer (`*Node`):**
`zero` is `nil`. Assigning it erases the pointer to the original `Node`.

**If `T` is a slice (`[]byte`):**
`zero` is a nil slice header. Assigning it removes the link to the old slice’s backing array.

**If `T` is a map:**
`zero` is a nil map. Removing it drops the reference to the map’s internal hash table.

**If `T` has no pointer fields (e.g., `int`, `float64`, or a fully value-only struct):**
Zeroing has no GC effect. These don’t reference other heap objects.

In short: the GC follows *actual pointers in memory*. Zeroing replaces a
pointer-bearing element with `nil`, removing that link. The slice re-slicing
alone doesn’t clear those pointers; writing the zero value does.


## ANKI LINE ------------------------

Q: In Go, when do you need to worry about the cost of `copy` with slices?

A:
Whenever you shift or duplicate large portions of the slice on each operation; repeated `copy` calls can turn what "should" be O(n) overall into O(n²).

Q: What standard library type in Go can also be used as a double-ended queue?

A:
`container/list.List`, a doubly linked list that gives O(1) `PushFront`, `PushBack`, and removal given a node pointer.

Q: Why is a slice-based queue preferred over `container/list` in many Go codebases?

A:
Slices are simpler, cache-friendly, and idiomatic. `container/list` is useful in niche cases (frequent middle removals, LRU lists), but slices are usually enough for queues.

---

## 005: Breadth First Search (BFS) Intro

Practice Problem:

You have an unweighted graph where each node is a string. The graph is stored as an adjacency list.

Q: If the nodes have type `string`, what is the standard Go type to represent the graph?

A:
```go
type Graph map[string][]string
```

Q: How do you initialize the queue for BFS, given a starting node `start` of type `string`?

A:
```go
q := []string{start}
```

Q: Do you need a visited set (or map) in BFS? Why?

A:
Yes. You need a visited map to avoid infinite loops in graphs with cycles and to avoid revisiting nodes unnecessarily.

Q: Will `BFS(graph, start)` visit nodes that are disconnected from `start`?

A:
No. BFS from `start` only visits nodes reachable from `start` via edges in the graph.

Q: How can you guarantee you've traversed the entire graph if it may be disconnected?

A:
Loop over all nodes in the graph; for each node that is not yet visited, run BFS from that node.

---

### Full BFS Implementation in Go

Q: Implement BFS traversal on this graph type:

```go
type Graph map[string][]string

// bfs returns all nodes reachable from start.
func bfs(g Graph, start string) map[string]bool {
    // TODO: implement
}
```

A:
```go
type Graph map[string][]string

// bfs returns all nodes reachable from start, including start itself.
func bfs(g Graph, start string) map[string]bool {
    visited := make(map[string]bool)
    q := []string{start}

    for len(q) > 0 {
        // Dequeue
        node := q[0]
        q = q[1:]

        if visited[node] {
            continue
        }

        visited[node] = true

        // Enqueue neighbors.
        neighbors := g[node]
        q = append(q, neighbors...)
    }

    return visited
}
```

Q: In this BFS, where is the queue's "enqueue" operation?

A:
`q = append(q, neighbors...)` at the end of the loop.

Q: In this BFS, where is the queue's "dequeue" operation?

A:
`node := q[0]; q = q[1:]` at the start of the loop.

Q: Why do we check `if visited[node] { continue }` after popping from the queue,
instead of before pushing neighbors?

A:
This pattern lets us freely enqueue neighbors multiple times without worrying
about duplicates at enqueue time. We centralize the "already processed?" check at
the point of dequeue. It's a common and simple pattern.

Q: What is the time complexity of BFS on a graph with `V` vertices and `E` edges
using the slice-based queue?

A:
O(V + E). Each vertex is enqueued/dequeued at most once, and each edge is
examined at most twice (for undirected graphs).

---

### Go Equivalents of Python Deque Ops (for Anki-style recall)

Use these as small flashcards.

Q: Python: `deque.append(x)` adds an item to the right. What is the closest Go idiom for queue enqueue?

A:
`q = append(q, x)` where `q` is a slice.

Q: Python: `deque.extend(iterable)` adds multiple items. What is the Go idiom?

A:
`q = append(q, items...)` where `items` is a slice.

Q: Python: `deque.popleft()` returns and removes the oldest item. What is the Go idiom?

A:

```go
x := q[0]
q = q[1:]
```

(or the variant that zeroes `q[0]` first for GC safety).




