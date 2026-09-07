#!/usr/bin/env -S uv run --with pyyaml python
"""Validate agent skill frontmatter."""

import re
import sys
import yaml
from pathlib import Path

def validate_skill(skill_path):
    """Basic validation of a skill"""
    skill_path = Path(skill_path)

    # Check SKILL.md exists
    skill_md = skill_path / 'SKILL.md'
    if not skill_md.exists():
        return False, "SKILL.md not found"

    # Read and validate frontmatter
    content = skill_md.read_text()
    if not content.startswith('---'):
        return False, "No YAML frontmatter found"

    # Extract frontmatter
    match = re.match(r'^---\n(.*?)\n---', content, re.DOTALL)
    if not match:
        return False, "Invalid frontmatter format"

    frontmatter_text = match.group(1)

    # Parse YAML frontmatter
    try:
        frontmatter = yaml.safe_load(frontmatter_text)
        if not isinstance(frontmatter, dict):
            return False, "Frontmatter must be a YAML dictionary"
    except yaml.YAMLError as e:
        return False, f"Invalid YAML in frontmatter: {e}"

    # Define allowed properties
    ALLOWED_PROPERTIES = {'name', 'description', 'license', 'metadata', 'compatibility'}

    # Check for unexpected properties (excluding nested keys under metadata)
    unexpected_keys = set(frontmatter.keys()) - ALLOWED_PROPERTIES
    if unexpected_keys:
        return False, (
            f"Unexpected key(s) in SKILL.md frontmatter: {', '.join(sorted(unexpected_keys))}. "
            f"Allowed properties are: {', '.join(sorted(ALLOWED_PROPERTIES))}"
        )

    # Check required fields
    if 'name' not in frontmatter:
        return False, "Missing 'name' in frontmatter"
    if 'description' not in frontmatter:
        return False, "Missing 'description' in frontmatter"

    # Classify from the resolved path. Public skills are distributed from
    # Unique-Divine/jiyuu under jiyuu/ai-skills and omit metadata.private.
    # Private skills are real directories beneath boku/priv-skills.
    # Runtime views (~/.agents/skills, ~/.cursor/skills, priv-skills
    # symlinks) follow to those canonical locations.
    metadata = frontmatter.get('metadata', {})
    if not isinstance(metadata, dict):
        return False, "Metadata must be a YAML dictionary"
    unexpected_metadata = set(metadata.keys()) - {
        'private', 'gh-repo', 'repo-dir'
    }
    if unexpected_metadata:
        return False, (
            f"Unexpected metadata key(s): {', '.join(sorted(unexpected_metadata))}. "
            "Allowed metadata properties are: private, gh-repo, repo-dir"
        )
    private = metadata.get('private')
    if private is not None and not isinstance(private, bool):
        return False, "metadata.private must be a boolean"
    gh_repo = metadata.get('gh-repo')
    if gh_repo is not None:
        if not isinstance(gh_repo, str):
            return False, "metadata.gh-repo must be a string"
        if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', gh_repo):
            return False, "metadata.gh-repo must use owner/repository form"
        if private is not True:
            return False, (
                "Skills with metadata.gh-repo must set metadata.private: true"
            )
    repo_dir = metadata.get('repo-dir')
    if repo_dir is not None:
        if not isinstance(repo_dir, str) or not repo_dir.strip():
            return False, "metadata.repo-dir must be a non-empty string"
        if gh_repo is None:
            return False, "metadata.repo-dir requires metadata.gh-repo"
        repo_dir_path = Path(repo_dir)
        if (
            repo_dir_path.is_absolute()
            or repo_dir_path == Path('.')
            or '..' in repo_dir_path.parts
        ):
            return False, "metadata.repo-dir must resolve beneath REPO"

    resolved_parts = skill_path.resolve().parts
    canonical_private = (
        'boku' in resolved_parts and 'priv-skills' in resolved_parts
    )
    canonical_public = (
        'boku' in resolved_parts
        and 'jiyuu' in resolved_parts
        and 'ai-skills' in resolved_parts
    )
    if canonical_private and private is not True:
        return False, "Skills in boku/priv-skills require metadata.private: true"
    if canonical_public and private is not None:
        return False, "Public skills in jiyuu/ai-skills must omit metadata.private"

    # Extract name for validation
    name = frontmatter.get('name', '')
    if not isinstance(name, str):
        return False, f"Name must be a string, got {type(name).__name__}"
    name = name.strip()
    if name:
        # Check naming convention (kebab-case: lowercase with hyphens)
        if not re.match(r'^[a-z0-9-]+$', name):
            return False, f"Name '{name}' should be kebab-case (lowercase letters, digits, and hyphens only)"
        if name.startswith('-') or name.endswith('-') or '--' in name:
            return False, f"Name '{name}' cannot start/end with hyphen or contain consecutive hyphens"
        # Check name length (max 64 characters per spec)
        if len(name) > 64:
            return False, f"Name is too long ({len(name)} characters). Maximum is 64 characters."

    # Extract and validate description
    description = frontmatter.get('description', '')
    if not isinstance(description, str):
        return False, f"Description must be a string, got {type(description).__name__}"
    description = description.strip()
    if description:
        # Check for angle brackets
        if '<' in description or '>' in description:
            return False, "Description cannot contain angle brackets (< or >)"
        # Check description length (max 1024 characters per spec)
        if len(description) > 1024:
            return False, f"Description is too long ({len(description)} characters). Maximum is 1024 characters."

    # Validate compatibility field if present (optional)
    compatibility = frontmatter.get('compatibility', '')
    if compatibility:
        if not isinstance(compatibility, str):
            return False, f"Compatibility must be a string, got {type(compatibility).__name__}"
        if len(compatibility) > 500:
            return False, f"Compatibility is too long ({len(compatibility)} characters). Maximum is 500 characters."

    return True, "Skill frontmatter is valid."

if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: python quick_validate.py <skill_directory>")
        sys.exit(1)
    
    valid, message = validate_skill(sys.argv[1])
    print(message)
    sys.exit(0 if valid else 1)
