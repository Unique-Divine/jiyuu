package aictx

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/suite"
)

func TestAictx(t *testing.T) {
	suite.Run(t, new(S))
}

type S struct {
	suite.Suite
}

func (s *S) TestFindGitRepo() {
	type TC struct {
		absPath string
		isRepo  bool
		wantErr string
	}
	for tcIdx, tc := range []TC{
		{absPath: "/foo", isRepo: false, wantErr: "No such file or directory"},
		{absPath: ".", isRepo: true, wantErr: ""},
		{absPath: "/dev", isRepo: false, wantErr: "fatal: not a git repository"},
	} {
		s.Run(fmt.Sprintf("tc %d, %#v", tcIdx, tc), func() {
			isRepo, repoPath, err := FindGitRepo(tc.absPath)
			s.Require().Equalf(tc.isRepo, isRepo, "repoPath: %q", repoPath)
			if len(tc.wantErr) != 0 {
				s.Require().ErrorContainsf(err, tc.wantErr, "repoPath: %q", repoPath)
				return
			}
			s.Require().NoErrorf(err, "repoPath: %q", repoPath)
		})
	}
}

// TestShouldIgnore_UsesRepoRootAndGitignore verifies that ShouldIgnore applies
// gitignore rules correctly using the repo root as the reference point.
//
// The test sets up an isolated fake repo containing:
//   - a .gitignore file with patterns: "dist/" and "*.log"
//   - three files: dist/main.js, server.log, and src/main.go
//
// Expected behavior:
//   - dist/main.js is ignored because it matches the "dist/" directory pattern
//   - server.log is ignored because it matches the "*.log" file pattern
//   - src/main.go is not ignored
//
// This test ensures three things:
//  1. LoadIgnoreLines correctly loads .gitignore from the repo root
//  2. BuildIgnorer compiles those lines into actionable patterns
//  3. ShouldIgnore properly converts absolute paths → repo-relative paths
//     before applying the gitignore matcher.
func (s *S) TestShouldIgnore_UsesRepoRootAndGitignore() {
	tmp := s.T().TempDir()

	// Fake repo root with a .gitignore.
	repoRoot := filepath.Join(tmp, "repo")
	s.Require().NoError(os.MkdirAll(repoRoot, 0o755))

	gi := "dist/\n*.log\n"
	s.Require().NoError(os.WriteFile(filepath.Join(repoRoot, ".gitignore"), []byte(gi), 0o644))

	// Files under repo
	distFile := filepath.Join(repoRoot, "dist", "main.js")
	logFile := filepath.Join(repoRoot, "server.log")
	srcFile := filepath.Join(repoRoot, "src", "main.go")

	for _, p := range []string{distFile, logFile, srcFile} {
		s.Require().NoError(os.MkdirAll(filepath.Dir(p), 0o755))
		s.Require().NoError(os.WriteFile(p, []byte("x"), 0o644))
	}

	rc := NewRunCtx()
	rc.Cwd = repoRoot
	rc.CwdRepoRoot = repoRoot
	s.Require().NoError(rc.LoadIgnoreLines())
	rc.BuildIgnorer()

	s.True(rc.ShouldIgnore(distFile), "dist should be ignored")
	s.True(rc.ShouldIgnore(logFile), "log should be ignored")
	s.False(rc.ShouldIgnore(srcFile), "src/main.go should not be ignored")
}

// TestStitchPath_RespectsGitignoreForDirs ensures that directory-level ignore
// patterns correctly prune traversal when stitching a directory tree.
//
// The test constructs a fake repo with:
//   - a .gitignore containing "dist/"
//   - a file in an ignored directory:   dist/ignore.go
//   - a file in a non-ignored directory: src/include.go
//
// When rc.stitchPath is called on the repo root, the output buffer should:
//   - include src/include.go
//   - omit dist/ignore.go
//
// This test confirms that:
//  1. directory ignore patterns cause traversal skipping (SkipDir semantics)
//  2. stitchPath and the WalkDir callback both enforce ShouldIgnore
//  3. no ignored files ever appear in stitched output.
func (s *S) TestStitchPath_RespectsGitignoreForDirs() {
	tmp := s.T().TempDir()
	repoRoot := filepath.Join(tmp, "repo")
	s.Require().NoError(os.MkdirAll(repoRoot, 0o755))

	// .gitignore
	s.Require().NoError(os.WriteFile(
		filepath.Join(repoRoot, ".gitignore"),
		[]byte("dist/\n"),
		0o644,
	))

	// Files
	distFile := filepath.Join(repoRoot, "dist", "ignore.go")
	srcFile := filepath.Join(repoRoot, "src", "include.go")

	for _, p := range []string{distFile, srcFile} {
		s.Require().NoError(os.MkdirAll(filepath.Dir(p), 0o755))
		s.Require().NoError(os.WriteFile(p, []byte("x"), 0o644))
	}

	rc := NewRunCtx()
	rc.Cwd = repoRoot
	rc.CwdRepoRoot = repoRoot
	s.Require().NoError(rc.LoadIgnoreLines())
	rc.BuildIgnorer()

	var buf bytes.Buffer
	s.Require().NoError(rc.stitchPath(&buf, repoRoot))

	out := buf.String()
	s.Contains(out, srcFile)
	s.NotContains(out, distFile)
}

func (s *S) TestDepthWithinRoot() {
	type TC struct {
		root string
		path string
		want int
	}

	tests := []TC{
		// Same directory
		{root: "/a/b", path: "/a/b", want: 0},

		// Direct children
		{root: "/a/b", path: "/a/b/c", want: 1},
		{root: "/a/b/", path: "/a/b/c", want: 1}, // trailing slash on root

		// Grandchildren
		{root: "/a/b", path: "/a/b/c/d", want: 2},

		// Sibling directory (depth becomes zero or negative depending on path shape)
		// In this case:
		//   root="/a/b" has 2 slashes, path="/a/x" also has 2 slashes → depth = 0
		{root: "/a/b", path: "/a/x", want: 0},
	}

	for i, tc := range tests {
		s.Run(fmt.Sprintf("tc %d (%s -> %s)", i, tc.root, tc.path), func() {
			got := depthWithinRoot(tc.root, tc.path)
			s.Equalf(tc.want, got, "root=%q path=%q", tc.root, tc.path)
		})
	}
}

func (s *S) TestStitchPath_RespectsLevelLimit() {
	tmp := s.T().TempDir()
	root := filepath.Join(tmp, "rootpath")
	s.Require().NoError(os.MkdirAll(root, 0o755))

	// Tree:
	//   root/
	//     a.txt             (depth 1)
	//     sub/b.txt         (depth 2)
	//     sub/deeper/c.txt  (depth 3)
	aFile := filepath.Join(root, "a.txt")
	subDir := filepath.Join(root, "sub")
	bFile := filepath.Join(subDir, "b.txt")
	deeperDir := filepath.Join(subDir, "deeper")
	cFile := filepath.Join(deeperDir, "c.txt")

	for _, p := range []string{aFile, bFile, cFile} {
		s.Require().NoError(os.MkdirAll(filepath.Dir(p), 0o755))
		s.Require().NoError(os.WriteFile(p, []byte("x"), 0o644))
	}

	// Helper to run stitchPath with a given level and return output as string.
	runWithLevel := func(level int) string {
		rc := NewRunCtx()
		rc.Cwd = root
		rc.CwdRepoRoot = root
		rc.Opts = &FlagOpts{Level: level}

		var buf bytes.Buffer
		s.Require().NoError(rc.stitchPath(&buf, root))
		return buf.String()
	}

	type TC struct {
		level          int
		name           string
		wantContains   []string
		wantNotContain []string
	}

	tests := []TC{
		{
			name:         "unlimited_-1",
			level:        -1,
			wantContains: []string{aFile, bFile, cFile},
		},
		{
			name:  "level_0_no_files_under_root",
			level: 0,
			// We don't expect any files printed, since all are below the root dir.
			wantNotContain: []string{aFile, bFile, cFile},
		},
		{
			name:           "level_1_only_direct_children",
			level:          1,
			wantContains:   []string{aFile},
			wantNotContain: []string{bFile, cFile},
		},
		{
			name:           "level_2_children_and_grandchildren",
			level:          2,
			wantContains:   []string{aFile, bFile},
			wantNotContain: []string{cFile},
		},
	}

	for _, tc := range tests {
		tc := tc
		s.Run(tc.name, func() {
			out := runWithLevel(tc.level)

			for _, p := range tc.wantContains {
				s.Contains(out, p, "expected %q to be included for level=%d", p, tc.level)
			}
			for _, p := range tc.wantNotContain {
				s.NotContains(out, p, "expected %q to be excluded for level=%d", p, tc.level)
			}
		})
	}
}
