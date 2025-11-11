# Depth-First Search (DFS)

- [Q: Is this bottom statement syntactically correct? If not, describe why and how to](#q-is-this-bottom-statement-syntactically-correct-if-not-describe-why-and-how-to)
- [Ex 0: Fill Image Pixels](#ex-0-fill-image-pixels)
- [Ex 1: Graph Traversal without Grid](#ex-1-graph-traversal-without-grid)
- [Ex 2: Number of Islands](#ex-2-number-of-islands)
- [Ex 3: Path Exists in Maze](#ex-3-path-exists-in-maze)
- [Ex 4: Count Connected Components in an Undirected Graph](#ex-4-count-connected-components-in-an-undirected-graph)
- [Ex 5: Image Fill in a 2D Grid](#ex-5-image-fill-in-a-2d-grid)
- [25-11-04](#25-11-04)

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


Q: We often say that DFS is implemented with a stack and BFS is implemented with a
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

A:
DFS?



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

- o - Two Sum
Use a hash map from value → index to find complements in O(n) time.

- o - Contains Duplicate / Contains Duplicate II / III
Basic set / map use: track what you’ve seen, or use a map from value → last index.

- o - Valid Anagram
Use a `map[byte]int` (or `[26]int`) to count characters.

- o - Group Anagrams
    Key idea: hash words by some canonical representation:
    - sort the string, or
    - count characters and use that as a key.

- o - Happy Number
Use a set to detect cycles in repeated transformations.

- o - Two Sum variants / 3Sum-closest-style helpers
Many can be viewed through “store something in a map as you scan” patterns.

- o - Subarray Sum Equals K
Use hashing on prefix sums: map from prefix_sum → count, detect ranges summing to K.

- o - Longest Substring Without Repeating Characters
Sliding window with a map[byte]int or map[rune]int for last seen indices.

- o - Top K Frequent Elements / Sort Characters by Frequency
Use a map[key]count, then bucket or sort.
