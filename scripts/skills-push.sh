#!/usr/bin/env bash
# Sync ai-skills: push from ~/.cursor/skills into repo ai-skills.
# Run via: just skills-push [--force] [--dry-run]

set -euo pipefail

print_usage() {
  cat <<'EOF'
Usage: skills-push.sh [--force] [--dry-run]

Push ~/.cursor/skills into repo ai-skills.

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

if [[ ! -d "$skills_dir" ]]; then
  echo "Expected local skills directory \"$skills_dir\" does not exist." >&2
  exit 1
fi

diff_exit_code=1
if [[ -d "$repo_skills_dir" ]]; then
  diff_exit_code=0
  diff -ruN "$skills_dir/" "$repo_skills_dir/" >/dev/null || diff_exit_code=$?
fi

if [[ $diff_exit_code -eq 0 ]]; then
  echo "AI skills already match. No sync needed."
  exit 0
fi

if [[ $diff_exit_code -gt 1 ]]; then
  echo "diff failed while comparing \"$skills_dir\" and \"$repo_skills_dir\"" >&2
  exit 2
fi

if [[ "$dry_run" == true ]]; then
  if [[ -d "$repo_skills_dir" || -f "$repo_skills_dir" ]]; then
    echo "Dry run: would replace \"$repo_skills_dir\" with \"$skills_dir\"."
  else
    echo "Dry run: would create \"$repo_skills_dir\" from \"$skills_dir\"."
  fi
  exit 0
fi

if [[ "$force" != true ]]; then
  echo "Differences found between \"$skills_dir\" and \"$repo_skills_dir\"." >&2
  echo "Run \`just skills-diff\` to inspect the changes." >&2
  echo "Run \`just skills-push --force\` to overwrite repo ai-skills from local Cursor skills." >&2
  echo "Run \`just skills-push --dry-run\` to preview the overwrite without changing files." >&2
  exit 1
fi

if [[ -d "$repo_skills_dir" || -f "$repo_skills_dir" ]]; then
  echo "Clearing old skills from \"$repo_skills_dir\""
  rm -rf "$repo_skills_dir"
fi

echo "Injecting skills from path $skills_dir"
cp -r "$skills_dir" "$repo_skills_dir"
