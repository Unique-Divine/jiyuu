#!/usr/bin/env bash
# View ai-skills diff: compare repo ai-skills against ~/.cursor/skills.
# Run via: just skills-diff

set -euo pipefail

print_usage() {
  cat <<'EOF'
Usage: skills-diff.sh

Print the recursive diff between repo ai-skills and ~/.cursor/skills.

Exit codes:
  0  No differences
  1  Differences found
  2  Error
EOF
}

if [[ $# -gt 0 ]]; then
  case "$1" in
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      print_usage >&2
      exit 2
      ;;
  esac
fi

skills_dir="${CURSOR_SKILLS_DIR:-$HOME/.cursor/skills}"
repo_skills_dir="${REPO_SKILLS_DIR:-ai-skills}"

if [[ ! -d "$repo_skills_dir" ]]; then
  echo "Expected repo skills directory \"$repo_skills_dir\" does not exist." >&2
  exit 2
fi

if [[ ! -e "$skills_dir" ]]; then
  echo "Local skills directory \"$skills_dir\" does not exist." >&2
  exit 1
fi

diff_exit_code=0
diff -ruN "$repo_skills_dir/" "$skills_dir/" || diff_exit_code=$?

if [[ $diff_exit_code -le 1 ]]; then
  exit $diff_exit_code
fi

echo "diff failed while comparing \"$repo_skills_dir\" and \"$skills_dir\"" >&2
exit 2
