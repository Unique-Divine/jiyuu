#!/usr/bin/env bash

# ------ Export Line

# assert attempts to run an arbitrary command and errors out of the script
# otherwise.
assert() {
  # Check if no arguments are passed
  if [ $# -eq 0 ]; then
    printf "assert (bash function): Executes a command and exits with an error message if it fails.\n\n"
    echo "Usage:"
    echo "    assert [cmd]"
    return
  fi

  local arg_cmd="$*"
  if ! eval "$arg_cmd"; then
    echo "Failed to execute command: $arg_cmd"
    return 1
  fi

  # local arg_cmd="$@"
  # if ! eval $arg_cmd; then
  #   echo "Failed to execute command: $arg_cmd"
  #   return 1
  # fi
}

# -------------------------------------------------
# COLORS: Terminal colors are set with ANSI escape codes.

export COLOR_GREEN="\033[32m"
export COLOR_CYAN="\033[36m"
export COLOR_RESET="\033[0m"

export COLOR_BLACK="\033[30m"
export COLOR_RED="\033[31m"
export COLOR_YELLOW="\033[33m"
export COLOR_BLUE="\033[34m"
export COLOR_MAGENTA="\033[35m"
export COLOR_WHITE="\033[37m"

# Bright color definitions
export COLOR_BRIGHT_BLACK="\033[90m"
export COLOR_BRIGHT_RED="\033[91m"
export COLOR_BRIGHT_GREEN="\033[92m"
export COLOR_BRIGHT_YELLOW="\033[93m"
export COLOR_BRIGHT_BLUE="\033[94m"
export COLOR_BRIGHT_MAGENTA="\033[95m"
export COLOR_BRIGHT_CYAN="\033[96m"
export COLOR_BRIGHT_WHITE="\033[97m"

# -------------------------------------------------
# LOGGING

# log_debug: Simple wrapper for `echo` with a DEBUG prefix.
log_debug() {
  echo -e "${COLOR_CYAN}DEBUG${COLOR_RESET}" "$@"
}

# log_error: ERROR messages in red, output to stderr.
log_error() {
  echo -e "❌ ${COLOR_RED}ERROR:${COLOR_RESET}" "$@" >&2
}

log_success() {
  echo -e "${COLOR_GREEN}INFO ✅:${COLOR_RESET}" "$@"
}

# log_warning: WARNING messages represent non-critical issues that might not
# require immediate action but should be noted as points of concern or failure.
log_warning() {
  echo -e "${COLOR_YELLOW}INFO${COLOR_RESET}" "$@" >&2
}

log_info() {
  echo -e "${COLOR_MAGENTA}INFO${COLOR_RESET}" "$@"
}

# -------------------------------------------------
# OK Suffix: Functions used for error handling or validating inputs.

# which_ok: Check if the given binary is in the $PATH or if it is something
# callable in a bash program.
# Returns code 0 on success and code 1 if the command fails.
which_ok() {

  # Runnable binary on $PATH? Ex: "jq", "bun", etc.
  # Alias? Ex: "ls" (I have it aliased to exa).
  # Built-in? Ex: "echo", "cd"
  if which "$1" >/dev/null 2>&1; then
    return 0
  fi

  # Function? An example for this is "nvm", which is a pure bash function.
  if type -a "$1" >/dev/null; then
    return 0
  fi

  log_error "$1 is not present in \$PATH"
  return 1
}

# source_ok (Function): Sources a bash script if it exists.
# Usage:  source_ok [bash_script]
source_ok() {
  local bash_script="$1"
  if test -r "$bash_script"; then
    # shellcheck disable=SC1090
    source "$bash_script"
  fi
}

env_var_ok() {
  local env_var_name="$1"
  local env_var_value

  if [[ ! "$env_var_name" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
    log_error "invalid env var name: $env_var_name"
    return 1
  fi

  eval "env_var_value=\"\${$env_var_name:-}\""
  if [[ -z "$env_var_value" ]]; then
    log_error "expected env var to be set: $env_var_name"
    return 1  # Return 1 to indicate error (variable is not set)
  else
    return 0  # Return 0 to indicate success (variable is set)
  fi
}

# env_vars_ok: Plural version of `env_var_ok`. This function accepts a list
# of strings that identify environment variables, checks if they have a
# non-empty value, and errors if that's the case.
#
# Example:
# env_vars_ok FROM GITHUB_KEY NPM_KEY
env_vars_ok() {
  for env_var in "$@"; do
    if ! env_var_ok "$env_var"; then
      return 1 # Return 1 to indicate error
    fi
  done
  return 0
}
