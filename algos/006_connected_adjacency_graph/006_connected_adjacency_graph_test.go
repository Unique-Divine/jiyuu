package algos

import (
	"testing"

	"github.com/stretchr/testify/suite"
)

func Sol6Recursive(graph map[string][]string) int {
	islands := 0
	nodesSeen := make(map[string]struct{})

	var dfsTraverse func(node string)
	dfsTraverse = func(node string) {
		if _, isSeen := nodesSeen[node]; isSeen {
			return
		}
		nodesSeen[node] = struct{}{}

		for _, connection := range graph[node] {
			dfsTraverse(connection)
		}
	}

	for rootNode := range graph {
		_, isSeen := nodesSeen[rootNode]
		if isSeen {
			continue
		}

		islands++
		dfsTraverse(rootNode)
	}

	return islands
}

func Sol6Iterative(graph map[string][]string) int {
	islands := 0
	nodesSeen := make(map[string]struct{})
	for rootNode := range graph {
		if _, isSeen := nodesSeen[rootNode]; isSeen {
			continue
		}

		islands++

		traverseStack := []string{rootNode}
		for len(traverseStack) > 0 {
			node := traverseStack[len(traverseStack)-1]
			traverseStack = traverseStack[:len(traverseStack)-1]

			if _, isSeen := nodesSeen[node]; isSeen {
				continue
			}
			nodesSeen[node] = struct{}{}
			traverseStack = append(traverseStack, graph[node]...)
		}
	}
	return islands
}

// Assumes you have a function with this signature somewhere:
//
//	func Sol6(graph map[string][]string) int
func (s *S) TestSol6() {
	tests := []struct {
		name  string
		graph map[string][]string
		want  int
	}{
		{
			name: "single node",
			graph: map[string][]string{
				"A": {},
			},
			want: 1, // One node is one connected component.
		},
		{
			name: "simple chain A-B-C",
			graph: map[string][]string{
				"A": {"B"},
				"B": {"A", "C"},
				"C": {"B"},
			},
			want: 1, // All nodes are reachable from each other.
		},
		{
			name: "two separate pairs",
			graph: map[string][]string{
				"A": {"B"},
				"B": {"A"},
				"C": {"D"},
				"D": {"C"},
			},
			want: 2, // Two disconnected components: {A,B} and {C,D}.
		},
		{
			name: "pair plus isolated node",
			graph: map[string][]string{
				"A": {"B"},
				"B": {"A"},
				"C": {},
			},
			want: 2, // Components: {A,B} and {C}.
		},
		{
			name: "triangle cycle A-B-C-A",
			graph: map[string][]string{
				"A": {"B", "C"},
				"B": {"A", "C"},
				"C": {"A", "B"},
			},
			want: 1, // A cycle is still a single component.
		},
		{
			name: "star centered at A",
			graph: map[string][]string{
				"A": {"B", "C", "D"},
				"B": {"A"},
				"C": {"A"},
				"D": {"A"},
			},
			want: 1, // All nodes connect through A.
		},
		{
			name: "duplicate edges",
			graph: map[string][]string{
				"A": {"B", "B"},
				"B": {"A"},
			},
			want: 1, // Duplicate adjacency entries shouldn't change the answer.
		},
		{
			name: "self loop and isolated node",
			graph: map[string][]string{
				"A": {"A"},
				"B": {},
			},
			want: 2, // Self-loop doesn't connect A to B; components are {A} and {B}.
		},
		{
			name: "all isolated nodes",
			graph: map[string][]string{
				"A": {},
				"B": {},
				"C": {},
			},
			want: 3, // Each isolated node is its own component.
		},
		{
			name: "mixed shapes and an isolate",
			graph: map[string][]string{
				"A": {"B"},
				"B": {"A", "C"},
				"C": {"B"},
				"D": {"E", "F"},
				"E": {"D"},
				"F": {"D"},
				"G": {},
			},
			want: 3, // Components: {A,B,C}, {D,E,F}, {G}.
		},
		{
			name: "looks disconnected but connected via bridge",
			graph: map[string][]string{
				"A": {"B"},
				"B": {"A", "D"},
				"C": {"D"},
				"D": {"C", "B"},
			},
			want: 1, // C connects to A via C-D-B-A, so it's one component.
		},
		{
			name: "square polygon A-B-C-D-A",
			graph: map[string][]string{
				"A": {"B", "D"},
				"B": {"A", "C"},
				"C": {"B", "D"},
				"D": {"A", "C"},
			},
			want: 1, // A 4-cycle is a single component.
		},
		{
			name: "example from prompt",
			graph: map[string][]string{
				"A": {"B"},
				"B": {"A"},
				"C": {},
				"D": {"E"},
				"E": {"D"},
			},
			want: 3, // Components: {A,B}, {C}, {D,E}.
		},
	}

	for _, tc := range tests {
		s.Run(tc.name, func() {
			got := Sol6Recursive(tc.graph)
			s.Require().Equal(tc.want, got, "recursive")

			got = Sol6Iterative(tc.graph)
			s.Require().Equal(tc.want, got, "iterative")
		})
	}
}

type S struct{ suite.Suite }

func Test(t *testing.T) {
	suite.Run(t, new(S))
}
