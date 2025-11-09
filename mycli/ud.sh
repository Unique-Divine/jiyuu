#!/usr/bin/env bash

_ud_help() {
  local help_text
  help_text=$(cat <<EOF
NAME:
   ud - CLI for convenient bash execution

USAGE:
   ud [global options] command [command options] [arguments...]

COMMANDS:
   go             Golang-specific commands
   quick, q, cfg  Core configuration commands and common jumps to editors
   rs             Rust-specific commands
   nibi           Nibiru-specific commands
   md             Markdown commands
   help, h        Shows a list of commands or help for one command

GLOBAL OPTIONS:
   --help, -h     Show help
EOF
)
  echo "$help_text"
}

# Command: "ud go"
_ud_go() {
  local sub="${1:-help}"
  case "$sub" in
    test-short|ts)
      _ud_go_run "go test ./... -short 2>&1 | grep -Ev 'no test|no statement'" "$@" ;;
    test-int|ti)
      _ud_go_run "go test ./... -run Integration 2>&1 | grep -Ev 'no test|no statement'" "$@" ;;
    test|t)
      _ud_go_run "go test ./... 2>&1 | grep -Ev 'no test|no statement'" "$@" ;;
    lint)
      _ud_go_run "golangci-lint run --allow-parallel-runners --fix" "$@" ;;
    cover-short|cs)
      _ud_go_cover "go test ./... -short -cover -coverprofile='temp.out' 2>&1 | grep -Ev 'no test|no statement'" ;;
    cover|c)
      _ud_go_cover "go test ./... -cover -coverprofile='temp.out' 2>&1 | grep -v 'no test' | grep -v 'no statement'" ;;
    help|-h|--help|"")
      local help_text
      help_text=$(cat <<EOF
USAGE:
   ud go [command] [--cmd]

COMMANDS:
   test-short, ts     Run Golang unit tests
   test-int, ti       Run Golang integration tests
   test, t            Run Golang tests
   lint               Run golangci-lint
   cover-short, cs    Run short tests and view coverage
   cover, c           Run full tests and view coverage

FLAGS:
   --cmd              Print the underlying command
   --help, -h         Show help for the this command
EOF
)
      echo "$help_text"
      ;;
    *)
      echo "Unknown go subcommand: $sub"
      _ud_go help
      return 1
      ;;
  esac
}

_ud_go_run() {
  local base_cmd="$1"; shift
  if [[ " $* " =~ " --cmd " ]]; then
    echo "$base_cmd"
  else
    echo "$base_cmd"
    eval "$base_cmd"
  fi
}

_ud_go_cover() {
  local test_cmd="$1"
  echo "$test_cmd"
  eval "$test_cmd"
  go tool cover -html="temp.out" -o coverage.html
  open coverage.html || explorer.exe coverage.html 2>/dev/null || echo "Coverage report generated."
}

# ------------ Subcommand: ud quick 

# Command: "ud quick"
_ud_quick() {
  local sub="${1:-help}"
  case "$sub" in
    notes)
      _ud_go_run "notes" "$@" ;;
    todos)
      _ud_go_run "todos" "$@" ;;
    out)
      _ud_go_run "nvim $HOME/ki/out.txt" "$@" ;;
    help|-h|--help|"")
      local help_text
      help_text=$(cat <<EOF
USAGE:
   ud quick [command]

DESCRIPTION:
   Quick jumps to open Neovim (nvim) to different working directories.
   In the below commands, "edit" means "open nvim with a certain working directory".

COMMANDS:
   cfg_nvim     Edit nvim (Neovim) config
   cfg_tmux     Edit tmux config
   dotf         Edit your dotfiles
   music        Opens the Windows file explorer to your music files
   myrc         Edit your zshrc config
   notes        Edit your notes workspac
   out          Edit temporary file at \$HOME/ki/out.txt
   todos        Edit your notes workspace with your text-based TODO-list open

FLAGS:
   --help, -h         Show help for the this command
EOF
)
      echo "$help_text"
      ;;
    *)
      echo "Unknown quick subcommand: $sub"
      _ud_quick help
      return 1
      ;;
  esac
}

# ------------ Subcommand: ud rs

# Command: "ud rs"
_ud_rs() {
  local sub="${1:-help}"
  case "$sub" in
    test-short|ts)
      local crate
      crate=$(grep -e "^name = " Cargo.toml | cut -d= -f2- | tr -d '"[:space:]')
      if [[ -n "$crate" ]]; then
        cmd="RUST_BACKTRACE=1 cargo test --package \"$crate\""
      else
        cmd="RUST_BACKTRACE=1 cargo test"
      fi
      _ud_rs_run "$cmd" "${@:2}"
      ;;

    test|t)
      _ud_rs_run "cargo test" "${@:2}"
      ;;

    fmt)
      _ud_rs_run "cp \"\$KOJIN_PATH/dotfiles/rustfmt.toml\" . && cargo fmt --all" "${@:2}"
      ;;

    tidy)
      local cmd="cargo b && ud rs lint && ud rs fmt"
      _ud_rs_run "$cmd" "${@:2}"
      ;;

    lint|clippy)
      _ud_rs_run "cargo clippy --fix --allow-dirty --allow-staged" "${@:2}"
      ;;

    clippy-check)
      _ud_rs_run "cargo clippy" "${@:2}"
      ;;

    help|-h|--help|"")
      local help_text
      help_text=$(cat <<EOF
USAGE:
   ud rs [command] [--cmd]

DESCRIPTION:
   Rust-specific commands for common Cargo, rustup, and rustc workflows.

COMMANDS:
   test-short, ts      Run tests for the current package
   test, t             Run all tests
   fmt                 Format using rustfmt
   tidy                Build, lint, and format in sequence
   lint, clippy        Run linter with fixes
   clippy-check        Run linter in check-only mode

FLAGS:
   --cmd              Print the underlying command
   --help, -h         Show help for the this command
EOF
)
      echo "$help_text"
      ;;
    *)
      echo "Unknown rs subcommand: $sub"
      _ud_rs help
      return 1
      ;;
  esac
}

_ud_rs_run() {
  local base_cmd="$1"; shift
  if [[ " $* " =~ " --cmd " ]]; then
    echo "$base_cmd"
  else
    echo "$base_cmd"
    eval "$base_cmd"
  fi
}

# ------------ Subcommand: ud md

# Command: "ud md"
_ud_md() {
  local sub="${1:-help}"
  case "$sub" in
    show)
      echo "Use command: markdown-preview"
      echo 'Install with: bun install -g @mryhryki/markdown-preview'
      ;;

    help|-h|--help|"")
      local help_text
      help_text=$(cat <<EOF
USAGE:
   ud md [command]

DESCRIPTION:
   Markdown utilities for previewing or formatting markdown files.

COMMANDS:
   show     Show markdown preview in browser (requires markdown-preview)

NOTES:
   To install the preview tool: bun install -g @mryhryki/markdown-preview

FLAGS:
   --help, -h         Show help for the this command
EOF
)
      echo "$help_text"
      ;;

    *)
      echo "Unknown md subcommand: $sub"
      _ud_md help
      return 1
      ;;
  esac
}

# ------------ Subcommand: ud nibi

RPC_NIBI_LOCAL="http://localhost:26657"
RPC_NIBI_TEST="https://rpc.archive.testnet-2.nibiru.fi:443"  # load balanced between nodes
RPC_NIBI_PROD="https://rpc.archive.nibiru.fi:443"
RPC_NIBI_DEV="https://rpc.devnet-3.nibiru.fi:443"
# RPC_NIBI_TEST="https://rpc.testnet-2.nibiru.fi:443"  # load balanced between nodes
# RPC_NIBI_PROD="https://rpc.nibiru.fi:443"
# ITN_RPC="https://rpc-1.itn-1.nibiru.fi:443"   # individual node

# Command: "ud nibi cfg"
_ud_nibi_cfg() {
  local sub="${1:-help}"
  echo "Usage: ud nibi cfg [local | prod | test | dev]"
  echo "Sets the Nibiru CLI config to one of the Nibiru blockchain networks."
  case "$sub" in
    help|-h|--help|"")
      local help_text
      help_text=$(cat <<EOF
USAGE:
   ud nibi cfg [network]

DESCRIPTION:
   Set Nibiru CLI config to a specific network.

COMMANDS:
   local       Local network (localnet)
   prod        Mainnet (cataclysm-1)
   test        Test network (testnet)
   dev         Development network (devnet)

FLAGS:
   --help, -h         Show help for the this command
EOF
)
      echo "$help_text"
      ;;
    local)
      local rpc_url="$RPC_NIBI_LOCAL"
      local chain_id="nibiru-localnet-0"
      nibid config node $rpc_url
      nibid config chain-id "$chain_id"
      nibid config broadcast-mode sync
      nibid config
      export RPC="$rpc_url"
      ;;
    prod)
      local rpc_url="$RPC_NIBI_PROD"
      nibid config node "$rpc_url"
      nibid config chain-id cataclysm-1
      nibid config broadcast-mode sync
      nibid config
      export RPC="$rpc_url"
      ;;
    test)
      local rpc_url="$RPC_NIBI_TEST"
      local chain_id="nibiru-testnet-2"
      nibid config node $rpc_url
      nibid config chain-id "$chain_id"
      nibid config broadcast-mode sync
      nibid config
      export RPC="$rpc_url"
      ;;
    dev)
      local rpc_url="$RPC_NIBI_DEV"
      nibid config node $rpc_url
      nibid config chain-id nibiru-devnet-3
      nibid config broadcast-mode sync
      nibid config
      export RPC="$rpc_url"
      ;;
    *)
      echo "Unknown cfg subcommand: $sub"
      _ud_nibi_cfg help
      return 1
      ;;
  esac
}


# Command: "ud nibi"
_ud_nibi() {
  local sub="${1:-help}"
  case "$sub" in
    cfg)
      _ud_nibi_cfg "${@:2}"
      ;;

    addrs)
      echo "$ADDR_VAL ADDR_VAL"
      echo "$ADDR_UD ADDR_UD"
      echo "$ADDR_DELPHI ADDR_DELPHI"
      echo "$FAUCET_WEB FAUCET_WEB"
      echo "$FAUCET_DISCORD FAUCET_DISCORD"
      ;;

    get-nibid|gn)
      local cmd='curl -s https://get.nibiru.fi/@v2.9.0! | bash'
      echo "$cmd"
      eval "$cmd"
      ;;

    help|-h|--help|"")
      local help_text
      help_text=$(cat <<EOF
USAGE:
   ud nibi [command]

DESCRIPTION:
   Nibiru-specific tools and config commands.

COMMANDS:
   cfg               Set CLI config to target a network
   addrs             Show common Nibiru addresses used in testing
   get-nibid, gn     Install nibid binary (via curl)

FLAGS:
   --help, -h         Show help for the this command
EOF
)
      echo "$help_text"
      ;;

    *)
      echo "Unknown nibi subcommand: $sub"
      _ud_nibi help
      return 1
      ;;
  esac
}

# ------------ main entry point

ud() {
  local cmd="${1:-help}"
  case "$cmd" in
    go) _ud_go "${@:2}" ;;
    rs) _ud_rs "${@:2}" ;;
    md) _ud_md "${@:2}" ;;
    nibi) _ud_nibi "${@:2}" ;;
    quick|q|cfg) _ud_quick "${@:2}" ;;
    help|-h|--help|"") _ud_help ;;
    *) echo -e "Unknown command: $cmd\n"; _ud_help ;;
  esac
}
