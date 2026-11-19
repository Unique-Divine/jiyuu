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
// installed archive so a repeat run is a no-op; --force reinstalls.

import { copyFile, mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises"
import { dirname, join, resolve } from "node:path"

import {
  cachedArchivePath,
  checkArchiveEntries,
  defaultCacheDir,
  errorMessage,
  extractArchive,
  formatTreeReport,
  type GrammarLock,
  listArchive,
  parseChecksums,
  parseLock,
  readOptional,
  stampMatches,
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
      `download of ${url} failed: HTTP ${response.status}. If the release ` +
        "does not exist yet, pass --from <grammars.tar.zst> (see README.md).",
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

export type FetchOutcome = "up-to-date" | "installed"

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

  const stampText = await readOptional(stampPath)
  const targetExists = (await readOptional(join(target, "MANIFEST.md"))) !== null
  if (!options.force && targetExists && stampMatches(stampText, lock.sha256)) {
    log(`Grammars already installed from ${lock.sha256.slice(0, 12)}; nothing to do`)
    return "up-to-date"
  }

  const archive = await obtainArchive(
    lock,
    options.cacheDir ?? defaultCacheDir(),
    options.from,
    options.download ?? httpDownload,
    log,
  )

  const problems = checkArchiveEntries(await listArchive(archive), lock.path)
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
    await writeFile(stampPath, `${lock.sha256}\n`)
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
