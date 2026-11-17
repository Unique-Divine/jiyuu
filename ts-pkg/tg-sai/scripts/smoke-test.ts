/**
 * Pack tg-sai, install the tarball into a throwaway Bun global directory, and
 * run the installed `tg-sai` command.
 *
 * A plain `bun add -g` writes to the real global directory. On machines where
 * that directory is tracked (for example, a dotfiles checkout), the install
 * rewrites the tracked package.json and bun.lock to point at a local tarball.
 * This script never touches the real global directory.
 */
import assert from "node:assert/strict"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

export interface IsolatedGlobal {
  /** Temporary directory that holds everything below. */
  root: string
  /** bunfig.toml that points `bun add --global` at `globalDir`. */
  config: string
  globalDir: string
  binDir: string
  /** Base environment plus the Bun global-install overrides. */
  env: Record<string, string | undefined>
}

/** bunfig.toml content that redirects global installs and their bins. */
export function bunfigText(globalDir: string, binDir: string): string {
  return (
    "[install]\n" +
    `globalDir = ${JSON.stringify(globalDir)}\n` +
    `globalBinDir = ${JSON.stringify(binDir)}\n`
  )
}

/** Paths and environment for a global install rooted at `root`. */
export function isolatedGlobal(
  root: string,
  baseEnv: Record<string, string | undefined>,
): IsolatedGlobal {
  const globalDir = join(root, "global")
  const binDir = join(root, "bin")
  return {
    root,
    config: join(root, "bunfig.toml"),
    globalDir,
    binDir,
    env: {
      ...baseEnv,
      BUN_INSTALL_GLOBAL_DIR: globalDir,
      BUN_INSTALL_BIN: binDir,
    },
  }
}

/** Every installed copy of `name` under `nodeModules`, as relative paths. */
export function installedCopies(nodeModules: string, name: string): string[] {
  const glob = new Bun.Glob(`**/${name}/package.json`)
  return [...glob.scanSync({ cwd: nodeModules, dot: true })].sort()
}

/** Expected `bun pm pack` file name for a scoped package. */
export function tarballName(name: string, version: string): string {
  return `${name.replace(/^@/, "").replace("/", "-")}-${version}.tgz`
}

function run(
  command: string[],
  cwd: string,
  env: Record<string, string | undefined>,
): string {
  const result = Bun.spawnSync(command, {
    cwd,
    env,
    stdout: "pipe",
    stderr: "pipe",
  })
  const stdout = result.stdout.toString()
  assert.equal(
    result.exitCode,
    0,
    `${command.join(" ")}\n${stdout}\n${result.stderr.toString()}`,
  )
  return stdout
}

/** Run a command with its output shown, failing on a nonzero exit. */
function show(
  command: string[],
  cwd: string,
  env: Record<string, string | undefined>,
): void {
  console.log(`$ ${command.join(" ")}`)
  const result = Bun.spawnSync(command, {
    cwd,
    env,
    stdout: "inherit",
    stderr: "inherit",
  })
  assert.equal(result.exitCode, 0, `${command.join(" ")} failed`)
}

async function main(): Promise<void> {
  const pkgDir = join(import.meta.dir, "..")
  const pkg = await Bun.file(join(pkgDir, "package.json")).json()
  const iso = isolatedGlobal(
    mkdtempSync(join(tmpdir(), "tg-sai-smoke-")),
    process.env,
  )
  try {
    writeFileSync(iso.config, bunfigText(iso.globalDir, iso.binDir))
    run(
      [process.execPath, "pm", "pack", "--destination", iso.root],
      pkgDir,
      iso.env,
    )
    const tarball = join(iso.root, tarballName(pkg.name, pkg.version))
    assert.ok(await Bun.file(tarball).exists(), `missing ${tarball}`)

    run(
      [process.execPath, "add", "--global", `--config=${iso.config}`, tarball],
      iso.root,
      iso.env,
    )
    // One @mtcute/core keeps the wasm that tg-sai initializes shared with
    // @mtcute/bun; a second copy breaks every Telegram connection.
    const cores = installedCopies(
      join(iso.globalDir, "node_modules"),
      "@mtcute/core",
    )
    assert.deepEqual(cores, ["@mtcute/core/package.json"])
    const executable = join(iso.binDir, "tg-sai")
    assert.equal(
      run([executable, "--version"], iso.root, iso.env).trim(),
      pkg.version,
    )
    show([executable, "--help"], iso.root, iso.env)
    show([executable, "health"], iso.root, iso.env)
    console.log(
      `${pkg.name}@${pkg.version}: packed global install passed smoke tests`,
    )
  } finally {
    rmSync(iso.root, { recursive: true, force: true })
  }
}

if (import.meta.main) await main()
