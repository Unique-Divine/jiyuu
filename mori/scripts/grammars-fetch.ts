#!/usr/bin/env bun
// Installs the pinned Tree-sitter grammars into internal/cbm/vendored/grammars.
//
// Usage:
//   bun scripts/grammars-fetch.ts [--from <grammars.tar.zst>] [--force]
//     [--cache-dir <dir>]
//
// The archive named by grammars.lock.json is taken from the cache
// (${XDG_CACHE_HOME:-~/.cache}/jiyuu-mori/grammars/<sha256>.tar.zst), from
// --from, or downloaded from the lock URL, and its sha256 is checked before
// anything is unpacked. After extraction every file is checked against the
// grammar lines of scripts/vendored-checksums.txt. A stamp file records the
// installed archive and the size and mtime of every file. A repeat run
// compares the tree with the stamp and hashes the license files; if anything
// differs it re-hashes the whole tree and reinstalls only when that fails.
// --force reinstalls.

import { copyFile, mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises"
import { dirname, join, resolve } from "node:path"

import {
  cachedArchivePath,
  checkArchiveEntries,
  checkMemberTypes,
  compareSnapshot,
  defaultCacheDir,
  errorMessage,
  extractArchive,
  formatStamp,
  formatTreeReport,
  type GrammarLock,
  listArchive,
  parseChecksums,
  parseLock,
  parseStamp,
  readOptional,
  sampleFiles,
  sha256File,
  snapshotTree,
  treeReportOk,
  verifyArchive,
  verifyTree,
} from "./grammars-lib.ts"

export const MORI_ROOT = resolve(import.meta.dir, "..")
/** Stamp lives outside internal/cbm/vendored, which security-vendored.sh scans. */
export const STAMP_NAME = ".grammars.stamp"

export interface FetchArgs {
  from?: string
  force: boolean
  cacheDir?: string
}

export function parseFetchArgs(argv: string[]): FetchArgs {
  const args: FetchArgs = { force: false }
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i]
    const value = (): string => {
      const next = argv[++i]
      if (next === undefined || next.startsWith("--")) {
        throw new Error(`${flag} needs a value`)
      }
      return next
    }
    if (flag === "--from") args.from = resolve(value())
    else if (flag === "--cache-dir") args.cacheDir = resolve(value())
    else if (flag === "--force") args.force = true
    else throw new Error(`unknown argument: ${flag}`)
  }
  return args
}

export type Downloader = (url: string, dest: string) => Promise<void>

export const httpDownload: Downloader = async (url, dest) => {
  const response = await fetch(url, { signal: AbortSignal.timeout(30 * 60_000) })
  if (!response.ok) {
    throw new Error(
      `download of ${url} failed: HTTP ${response.status}. To install ` +
        "without the network, pass --from <grammars.tar.zst> (see README.md).",
    )
  }
  await Bun.write(dest, response)
}

export interface FetchOptions {
  root: string
  from?: string
  force?: boolean
  cacheDir?: string
  download?: Downloader
  log?: (line: string) => void
}

export type FetchOutcome = "up-to-date" | "reverified" | "installed"

/** Writes the stamp for the verified tree at `target`. */
async function writeStamp(stampPath: string, target: string, sha256: string): Promise<void> {
  const { files } = await snapshotTree(target)
  await writeFile(stampPath, formatStamp({ archiveSha256: sha256, files }))
}

/**
 * Checks an installed tree against its stamp without hashing everything:
 * the file list, each file's size and mtime, and the sha256 of MANIFEST.md
 * and every license file. Returns the problems found.
 */
export async function quickCheck(
  target: string,
  stampText: string | null,
  sha256: string,
  expected: Map<string, string>,
): Promise<string[]> {
  const stamp = parseStamp(stampText)
  if (stamp === null) return ["no stamp, or a stamp from an older fetch script"]
  if (stamp.archiveSha256 !== sha256) return ["installed from a different archive"]
  const problems = compareSnapshot(stamp, await snapshotTree(target), expected)
  if (problems.length > 0) return problems
  for (const rel of sampleFiles(expected)) {
    if ((await sha256File(join(target, rel))) !== expected.get(rel)) {
      problems.push(`wrong sha256: ${rel}`)
    }
  }
  return problems
}

/**
 * Returns a verified archive path in the cache, copying --from or
 * downloading when the cache has no valid copy.
 */
export async function obtainArchive(
  lock: GrammarLock,
  cacheDir: string,
  from: string | undefined,
  download: Downloader,
  log: (line: string) => void,
): Promise<string> {
  const cached = cachedArchivePath(cacheDir, lock.sha256)
  try {
    // Re-verify on every reuse: a cache entry is trusted only by its hash.
    await verifyArchive(cached, lock)
    log(`Using cached archive ${cached}`)
    return cached
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
      log(`Discarding cached archive: ${errorMessage(err)}`)
      await rm(cached, { force: true })
    }
  }
  await mkdir(cacheDir, { recursive: true })
  const staging = await mkdtemp(join(cacheDir, ".incoming-"))
  try {
    const partial = join(staging, "grammars.tar.zst")
    if (from !== undefined) {
      log(`Copying ${from}`)
      await copyFile(from, partial)
    } else {
      log(`Downloading ${lock.url}`)
      await download(lock.url, partial)
    }
    try {
      await verifyArchive(partial, lock)
    } catch (err) {
      throw new Error(`refusing to unpack: ${errorMessage(err)}`)
    }
    await rename(partial, cached)
    return cached
  } finally {
    await rm(staging, { recursive: true, force: true })
  }
}

export async function fetchGrammars(options: FetchOptions): Promise<FetchOutcome> {
  const log = options.log ?? ((line: string) => console.log(line))
  const root = options.root
  const lock = parseLock(await readFile(join(root, "grammars.lock.json"), "utf8"))
  const target = join(root, lock.path)
  const stampPath = join(root, STAMP_NAME)
  const expected = parseChecksums(
    await readFile(join(root, "scripts", "vendored-checksums.txt"), "utf8"),
    lock.path,
  )
  if (expected.size === 0) {
    throw new Error(`scripts/vendored-checksums.txt lists no files under ${lock.path}`)
  }

  const targetExists = (await readOptional(join(target, "MANIFEST.md"))) !== null
  if (!options.force && targetExists) {
    const problems = await quickCheck(
      target,
      await readOptional(stampPath),
      lock.sha256,
      expected,
    )
    if (problems.length === 0) {
      log(`Grammars already installed from ${lock.sha256.slice(0, 12)}; nothing to do`)
      return "up-to-date"
    }
    // Something moved since the last check (or the stamp predates it):
    // hash the whole tree before deciding to reinstall.
    log(`Re-verifying installed grammars: ${problems.slice(0, 3).join("; ")}`)
    const report = await verifyTree(target, expected)
    if (treeReportOk(report)) {
      await writeStamp(stampPath, target, lock.sha256)
      log(`Installed grammars match vendored-checksums.txt (${report.checked} files)`)
      return "reverified"
    }
    log(`Reinstalling: ${formatTreeReport(report)}`)
  }

  const archive = await obtainArchive(
    lock,
    options.cacheDir ?? defaultCacheDir(),
    options.from,
    options.download ?? httpDownload,
    log,
  )

  const members = await listArchive(archive)
  const problems = [
    ...checkArchiveEntries(
      members.map((member) => member.name),
      lock.path,
    ),
    ...checkMemberTypes(members),
  ]
  if (problems.length > 0) {
    throw new Error(`refusing to unpack ${archive}:\n  ${problems.slice(0, 20).join("\n  ")}`)
  }

  // Extract beside the target (same filesystem) so the final move is a rename.
  const staging = await mkdtemp(join(root, ".grammars-tmp-"))
  try {
    log(`Unpacking into ${lock.path}`)
    await extractArchive(archive, staging)
    const unpacked = join(staging, lock.path)
    const report = await verifyTree(unpacked, expected)
    if (!treeReportOk(report)) {
      throw new Error(
        `unpacked grammars do not match scripts/vendored-checksums.txt: ${formatTreeReport(report)}`,
      )
    }
    await rm(stampPath, { force: true })
    await rm(target, { recursive: true, force: true })
    await mkdir(dirname(target), { recursive: true })
    await rename(unpacked, target)
    await writeStamp(stampPath, target, lock.sha256)
    log(`Installed ${report.checked} grammar files (sha256 ${lock.sha256.slice(0, 12)})`)
    return "installed"
  } finally {
    await rm(staging, { recursive: true, force: true })
  }
}

if (import.meta.main) {
  try {
    const args = parseFetchArgs(process.argv.slice(2))
    await fetchGrammars({ root: MORI_ROOT, ...args })
  } catch (err) {
    console.error(`grammars-fetch: ${errorMessage(err)}`)
    process.exit(1)
  }
}
