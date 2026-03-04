# Ex 004: Path Exists in Maze

Practice Problem:

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

Q: 

Check if start is blocked? If so, return False.
Check if end is blocked. If so, return False.
Beginning from the start, 
DFS(up, left, down, right)

