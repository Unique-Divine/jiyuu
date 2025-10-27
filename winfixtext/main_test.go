package main

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestReplacer(t *testing.T) {
	testCases := []struct {
		given string
		want  string
	}{
		{
			given: "Why ItΓÇÖs Attractive",
			want:  "Why It's Attractive",
		},
		{
			given: "Why It╬ô├ç├ûs Attractive",
			want:  "Why It's Attractive",
		},
		{
			given: `that’s`,
			want:  `that's`,
		},
		{
			given: `xxxΓÇôyyy`,
			want:  `xxx-yyy`,
		},
		{
			given: `“xxyy”`,
			want:  `"xxyy"`,
		},
	}

	for _, tc := range testCases {
		got := REPLACER.Replace(tc.given)
		assert.Equal(t, tc.want, got)
	}

}

func TestProcessStream_InMemory(t *testing.T) {
	in := "Why It╬ô├ç├ûs Attractive\nΓÇ£xΓÇ¥\nlast line no newline"
	want := "Why It's Attractive\n\"x\"\nlast line no newline\n"

	var out bytes.Buffer
	err := ProcessStream(bytes.NewBufferString(in), &out)
	require.NoError(t, err, "ProcessStream error")

	got := out.String()
	require.Equal(t, want, got)
}

func TestFixFileInPlace_EndToEnd(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "sample.txt")

	orig := "thatΓÇÖs fine\nxxx╬ô├ç├┤yyy\nno newline at end"
	if err := os.WriteFile(path, []byte(orig), 0o644); err != nil {
		t.Fatalf("write seed file: %v", err)
	}

	if err := FixFileInPlace(path); err != nil {
		t.Fatalf("FixFileInPlace error: %v", err)
	}

	gotBytes, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read result: %v", err)
	}
	got := string(gotBytes)

	want := "that's fine\nxxx-yyy\nno newline at end\n"
	if got != want {
		t.Fatalf("got:\n%q\nwant:\n%q", got, want)
	}
}
