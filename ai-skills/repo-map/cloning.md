# Cloning — `$HOME/ki` layout (NibiruChain and related)

Use this when recreating the machine layout under **`$HOME/ki`**. All **NibiruChain** URLs use SSH; ensure `git@github.com` works for your keys.

## Discover NibiruChain slugs

```bash
gh repo list NibiruChain --limit 500 --json name --jq '.[].name' | sort
```

If the list truncates, raise `--limit` or run `gh api orgs/NibiruChain/repos --paginate`.

## NibiruChain — `git clone` (slug → local folder)

From an empty or partial `~/ki`, run only the lines you still need (skip if the directory already exists).

```bash
mkdir -p "$HOME/ki"
cd "$HOME/ki"

git clone git@github.com:NibiruChain/nibiru.git nibi-chain
git clone git@github.com:NibiruChain/go-ethereum.git nibi-geth
git clone git@github.com:NibiruChain/heart-monitor.git nibi-go-hm
git clone git@github.com:NibiruChain/ts-sdk.git nibi-ts-sdk
git clone git@github.com:NibiruChain/home-site.git nibi-home-site
git clone git@github.com:NibiruChain/web-app.git sai-website
git clone git@github.com:NibiruChain/sai-perps.git sai-perps
git clone git@github.com:NibiruChain/sai-keeper.git sai-keeper
git clone git@github.com:NibiruChain/sai-docs.git sai-docs
git clone git@github.com:NibiruChain/iac.git nibi-iac
```

The **`nibiru`** checkout also contains **`nibi-chain/sai-trading/`** (trading bot automation); there is **no** separate clone for that path.

Sanity-check: `git -C ~/ki/<folder> remote get-url origin` should show `NibiruChain/<slug>.git`; the **`folder`** name is chosen for ergonomics and may not equal **`slug`** (see table in [SKILL.md](./SKILL.md)).

## Wasm / toolchain (upstream CosmWasm, not NibiruChain)

```bash
mkdir -p "$HOME/ki"
cd "$HOME/ki"

git clone git@github.com:CosmWasm/cosmwasm.git wasm-cosmwasm
git clone git@github.com:CosmWasm/wasmvm.git wasm-go-wasmvm
```

## Personal repos (examples)

These are **outside** `NibiruChain`; URLs follow your forks.

```bash
cd "$HOME/ki"

git clone git@github.com:Unique-Divine/boku.git boku

# gh-io-ud: replace with your fork if different
git clone git@github.com:Unique-Divine/gh-io-ud.git gh-io-ud
```

## Optional: idempotent **`clone`** function

Runs the same clones as above; skips directories that already contain **`.git`**. Paste into **`~/.zshrc`** or run in a subshell:

```bash
clone_nibiruchain_ki() {
  mkdir -p "$HOME/ki" || return 1
  (
    cd "$HOME/ki" || return 1
    clone_into() {
      local slug="$1" dir="$2"
      if [[ -d "$dir/.git" ]]; then
        echo "skip existing: $dir"
        return 0
      fi
      git clone "git@github.com:NibiruChain/${slug}.git" "$dir"
    }
    clone_into nibiru               nibi-chain
    clone_into go-ethereum           nibi-geth
    clone_into heart-monitor         nibi-go-hm
    clone_into ts-sdk                nibi-ts-sdk
    clone_into home-site             nibi-home-site
    clone_into web-app               sai-website
    clone_into sai-perps             sai-perps
    clone_into sai-keeper            sai-keeper
    clone_into sai-docs              sai-docs
    clone_into iac                   nibi-iac
  )
}
```

Then **`clone_nibiruchain_ki`**.

## Remote check (one-liner)

```bash
for d in nibi-chain nibi-geth nibi-go-hm nibi-ts-sdk nibi-home-site sai-website sai-perps sai-keeper sai-docs nibi-iac; do
  printf '%s: ' "$d"
  git -C "$HOME/ki/$d" remote get-url origin 2>/dev/null || echo "(missing)"
done
```
