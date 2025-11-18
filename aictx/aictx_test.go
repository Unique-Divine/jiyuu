package main

import (
	"fmt"
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
