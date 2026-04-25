#!/usr/bin/env bash
# Sync ai-skills: pull from repo ai-skills into ~/.cursor/skills.
# Run via: just skills-pull [--force] [--dry-run]

set -euo pipefail

print_usage() {
  cat <<'EOF'
Usage: skills-pull.sh [--force] [--dry-run]

Pull repo ai-skills into ~/.cursor/skills.

Options:
  -f, --force    Overwrite without prompting
  -n, --dry-run  Preview the overwrite without changing files
  -h, --help     Show this help text
EOF
}

force=false
dry_run=false

for arg in "$@"; do
  case "$arg" in
    -f|--force)
      force=true
      ;;
    -n|--dry-run)
      dry_run=true
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $arg" >&2
      print_usage >&2
      exit 2
      ;;
  esac
done

skills_dir="${CURSOR_SKILLS_DIR:-$HOME/.cursor/skills}"
repo_skills_dir="${REPO_SKILLS_DIR:-ai-skills}"

if [[ ! -d "$repo_skills_dir" ]]; then
  echo "Expected repo skills directory \"$repo_skills_dir\" does not exist." >&2
  exit 1
fi

diff_exit_code=1
if [[ -d "$skills_dir" ]]; then
  diff_exit_code=0
  diff -ruN "$repo_skills_dir/" "$skills_dir/" >/dev/null || diff_exit_code=$?
fi

if [[ $diff_exit_code -eq 0 ]]; then
  echo "AI skills already match. No sync needed."
  exit 0
fi

if [[ $diff_exit_code -gt 1 ]]; then
  echo "diff failed while comparing \"$repo_skills_dir\" and \"$skills_dir\"" >&2
  exit 2
fi

if [[ "$dry_run" == true ]]; then
  if [[ -d "$skills_dir" || -f "$skills_dir" ]]; then
    echo "Dry run: would replace \"$skills_dir\" with \"$repo_skills_dir\"."
  else
    echo "Dry run: would create \"$skills_dir\" from \"$repo_skills_dir\"."
  fi
  exit 0
fi

if [[ "$force" != true ]]; then
  echo "Differences found between \"$repo_skills_dir\" and \"$skills_dir\"." >&2
  echo "Run \`just skills-diff\` to inspect the changes." >&2
  echo "Run \`just skills-pull --force\` to overwrite local Cursor skills from the repo." >&2
  echo "Run \`just skills-pull --dry-run\` to preview the overwrite without changing files." >&2
  exit 1
fi

if [[ -d "$skills_dir" || -f "$skills_dir" ]]; then
  echo "Clearing old skills from \"$skills_dir\""
  rm -rf "$skills_dir"
fi

echo "Injecting skills from path $repo_skills_dir"
cp -r "$repo_skills_dir" "$skills_dir"
