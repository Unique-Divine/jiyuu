#!/usr/bin/env bun
// Packs internal/cbm/vendored/grammars at a source commit into a
// deterministic grammars.tar.zst and prints its sha256 and size.
//
// Usage:
//   bun scripts/grammars-pack.ts --repo <git repo> [--commit <sha>]
//     [--out <file>] [--update-lock (--release-tag <tag> | --url <url>)]
//
// --commit defaults to sourceCommit in grammars.lock.json. --out defaults to
// $TMPDIR/jiyuu-mori/grammars.tar.zst and must lie outside the repository.
// --update-lock writes the new url, sha256, size and commit into the lock.
// It needs the release the archive will be published under: --release-tag
// swaps the tag in the lock's GitHub release URL (for example
// mori/grammars/v2), and --url sets any https URL.

import { mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path"

import {
  errorMessage,
  parseLock,
  runCommand,
  runPipeline,
  sha256File,
  ZSTD_COMPRESS_ARGS,
} from "./grammars-lib.ts"

export const MORI_ROOT = resolve(import.meta.dir, "..")
export const LOCK_PATH = join(MORI_ROOT, "grammars.lock.json")

export interface PackArgs {
  repo: string
  commit?: string
  out: string
  updateLock: boolean
  releaseTag?: string
  url?: string
}

const RELEASE_URL_RE = /^(https:\/\/github\.com\/[^/]+\/[^/]+\/releases\/download\/)[^/]+(\/[^/]+)$/

/**
 * The lock URL for `tag`: the lock's GitHub release download URL with its
 * tag segment replaced, keeping the asset name.
 */
export function releaseUrl(currentUrl: string, tag: string): string {
  const match = RELEASE_URL_RE.exec(currentUrl)
  if (!match) {
    throw new Error(
      `cannot derive a release URL from ${currentUrl}; pass --url instead`,
    )
  }
  if (tag.trim() === "" || tag !== tag.trim()) {
    throw new Error(`invalid release tag: ${JSON.stringify(tag)}`)
  }
  return `${match[1]}${encodeURIComponent(tag)}${match[2]}`
}

/** The lock JSON after a pack: new url, sha256, size and source commit. */
export function updatedLock(
  raw: Record<string, unknown>,
  update: { url: string; sha256: string; sizeBytes: number; commit: string },
): Record<string, unknown> {
  const next = {
    ...raw,
    url: update.url,
    sha256: update.sha256,
    sizeBytes: update.sizeBytes,
    sourceCommit: update.commit,
  }
  // Fails on a bad URL, hash or commit before anything is written.
  parseLock(JSON.stringify(next))
  return next
}

export function parsePackArgs(argv: string[]): PackArgs {
  const args: Partial<PackArgs> = { updateLock: false }
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i]
    const value = (): string => {
      const next = argv[++i]
      if (next === undefined || next.startsWith("--")) {
        throw new Error(`${flag} needs a value`)
      }
      return next
    }
    if (flag === "--repo") args.repo = resolve(value())
    else if (flag === "--commit") args.commit = value()
    else if (flag === "--out") args.out = resolve(value())
    else if (flag === "--update-lock") args.updateLock = true
    else if (flag === "--release-tag") args.releaseTag = value()
    else if (flag === "--url") args.url = value()
    else throw new Error(`unknown argument: ${flag}`)
  }
  if (!args.repo) throw new Error("--repo <path to codebase-memory-mcp> is required")
  if (args.releaseTag !== undefined && args.url !== undefined) {
    throw new Error("pass --release-tag or --url, not both")
  }
  if ((args.releaseTag !== undefined || args.url !== undefined) && !args.updateLock) {
    throw new Error("--release-tag and --url only apply with --update-lock")
  }
  if (args.updateLock && args.releaseTag === undefined && args.url === undefined) {
    throw new Error(
      "--update-lock needs --release-tag <tag> or --url <url> for the new archive",
    )
  }
  return {
    repo: args.repo,
    commit: args.commit,
    out: args.out ?? join(tmpdir(), "jiyuu-mori", "grammars.tar.zst"),
    updateLock: args.updateLock ?? false,
    releaseTag: args.releaseTag,
    url: args.url,
  }
}

/** True when `path` is `dir` or inside it. */
export function isInside(path: string, dir: string): boolean {
  const rel = relative(resolve(dir), resolve(path))
  return rel === "" || (rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel))
}

/**
 * Checks a grammar tree listing: MANIFEST.md must exist and every grammar
 * directory must carry a LICENSE. Returns the problems found.
 */
export function checkGrammarListing(paths: string[], root: string): string[] {
  const prefix = `${root}/`
  const dirs = new Set<string>()
  const licensed = new Set<string>()
  let manifest = false
  for (const path of paths) {
    if (!path.startsWith(prefix)) continue
    const rel = path.slice(prefix.length)
    if (rel === "MANIFEST.md") manifest = true
    const parts = rel.split("/")
    if (parts.length < 2) continue
    dirs.add(parts[0]!)
    if (parts.length === 2 && parts[1] === "LICENSE") licensed.add(parts[0]!)
  }
  const problems: string[] = []
  if (!manifest) problems.push(`${root}/MANIFEST.md is missing`)
  if (dirs.size === 0) problems.push(`no grammar directories under ${root}`)
  for (const dir of [...dirs].sort()) {
    if (!licensed.has(dir)) problems.push(`${root}/${dir}/LICENSE is missing`)
  }
  return problems
}

async function main(): Promise<void> {
  const args = parsePackArgs(process.argv.slice(2))
  const lock = parseLock(await readFile(LOCK_PATH, "utf8"))
  // Resolve the new URL first so a bad tag fails before the slow pack.
  const newUrl =
    args.url ??
    (args.releaseTag !== undefined ? releaseUrl(lock.url, args.releaseTag) : lock.url)
  const toplevel = (
    await runCommand("git", ["rev-parse", "--show-toplevel"], { cwd: MORI_ROOT })
  ).trim()
  if (isInside(args.out, toplevel) || isInside(args.out, MORI_ROOT)) {
    throw new Error(`--out must be outside the repository: ${args.out}`)
  }
  const commit = (
    await runCommand(
      "git",
      ["rev-parse", "--verify", `${args.commit ?? lock.sourceCommit}^{commit}`],
      { cwd: args.repo },
    )
  ).trim()
  const listing = (
    await runCommand("git", ["ls-tree", "-r", "--name-only", commit, lock.path], {
      cwd: args.repo,
    })
  )
    .split("\n")
    .filter(Boolean)
  const problems = checkGrammarListing(listing, lock.path)
  if (problems.length > 0) {
    throw new Error(`grammar tree at ${commit} is incomplete:\n  ${problems.join("\n  ")}`)
  }

  await mkdir(dirname(args.out), { recursive: true })
  const partial = `${args.out}.partial`
  await rm(partial, { force: true })
  console.log(`Packing ${listing.length} files from ${lock.path} at ${commit}`)
  // git archive stamps every entry with the commit time, so the tar stream is
  // a pure function of the commit; zstd settings are fixed in grammars-lib.
  await runPipeline(
    ["git", ["archive", "--format=tar", commit, lock.path]],
    ["zstd", [...ZSTD_COMPRESS_ARGS, "-o", partial]],
    { cwd: args.repo },
  )
  await rename(partial, args.out)
  const sha256 = await sha256File(args.out)
  const sizeBytes = (await stat(args.out)).size
  console.log(`archive:     ${args.out}`)
  console.log(`sha256:      ${sha256}`)
  console.log(`sizeBytes:   ${sizeBytes}`)
  console.log(`commit:      ${commit}`)
  if (args.updateLock) {
    const raw = JSON.parse(await readFile(LOCK_PATH, "utf8")) as Record<string, unknown>
    const next = updatedLock(raw, { url: newUrl, sha256, sizeBytes, commit })
    await writeFile(LOCK_PATH, `${JSON.stringify(next, null, 2)}\n`)
    console.log(`updated ${relative(process.cwd(), LOCK_PATH)}; publish the archive at ${newUrl}`)
  }
}

if (import.meta.main) {
  main().catch((err) => {
    console.error(`grammars-pack: ${errorMessage(err)}`)
    process.exit(1)
  })
}
