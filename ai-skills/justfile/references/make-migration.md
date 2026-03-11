# Makefile To Justfile Migration

Use this when a repo has both `Makefile` and `justfile`, or when the user wants
to migrate commands from `make` to `just`.

## Fast Mapping

| Make | Just | Notes |
| --- | --- | --- |
| `.PHONY: target` | not needed | `just` does not track file timestamps |
| `.DEFAULT_GOAL := help` | put the default recipe first or use `[default]` | first public recipe is the default |
| `target: dep1 dep2` | `target: dep1 dep2` | dependency syntax is similar |
| `@command` | `@command` | suppress command echo |
| `$(VAR)` / `${VAR}` | `{{var}}` | `just` interpolation |
| `$$VAR` | `$VAR` | shell vars do not use double-dollar escaping |
| `VAR := value` | `var := "value"` | quote strings in `just` |
| `VAR ?= default` | `var := env('VAR', 'default')` | env fallback pattern |
| `$(shell cmd)` | `` `cmd` `` | capture command output |
| `export VAR := value` | `export var := "value"` or `set export` | export explicitly or globally |
| `include file.mk` | `import 'file.just'` | import another file |

## Differences That Matter

### Shell Variables

In `make`, literal shell variables use `$$`. In `just`, use `$` directly.

```make
target:
	echo $$HOME
```

```just
target:
	echo $HOME
```

### Recipe Execution Model

Like `make`, each line runs in a separate shell. Use a shebang recipe when the
body must run as one script.

```just
check:
    #!/usr/bin/env bash
    if [ -f .env ]; then
        echo "found"
    fi
```

### Documentation

`make` often needs a custom help target. In `just`, comments above recipes are
already used by `just --list`.

```just
[private]
default:
    @just --list --unsorted

# Run unit tests
test:
    cargo test
```

### Arguments

`just` supports recipe arguments directly.

```just
recreate resource:
    terraform apply -replace "{{resource}}"
```

## Migration Checklist

1. Read the existing `Makefile` before rewriting anything.
2. Keep both files during migration if the repo still depends on `make`.
3. Convert variables, then recipes, then dependencies.
4. Replace `$$` shell variables with `$`.
5. Use comments above recipes so `just --list` stays useful.
6. Add `[confirm]` to destructive recipes if the old flow relied on manual care.
7. Verify migrated commands with `just --show <recipe>` and targeted dry runs.

## Agent Guardrails

- Do not suggest migration until you confirm the repo is using `just` as a real
  command surface rather than as an experiment.
- Watch for make-specific behavior that may not carry over cleanly, such as
  pattern rules, file-based dependency graphs, or job parallelism.
- If both files exist, prefer the one the repo actually documents and uses.
