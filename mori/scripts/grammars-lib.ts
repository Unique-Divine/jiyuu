// Shared logic for the pinned Tree-sitter grammar archive: lock parsing,
// sha256 checks, archive path safety, the extraction stamp and the
// vendored-checksums comparison. grammars-pack.ts and grammars-fetch.ts are
// thin command-line wrappers around these functions.

import { spawn } from "node:child_process"
import { createHash } from "node:crypto"
import { createReadStream } from "node:fs"
import { lstat, readdir, readFile } from "node:fs/promises"
import { homedir } from "node:os"
import { join, posix } from "node:path"

/** Contents of mori/grammars.lock.json. */
export interface GrammarLock {
  url: string
  sha256: string
  sizeBytes: number
  sourceRepository: string
  sourceCommit: string
  path: string
}

const SHA256_RE = /^[0-9a-f]{64}$/
const COMMIT_RE = /^[0-9a-f]{40}$/

/** Decompression flags shared by every reader of the archive. */
export const ZSTD_DECOMPRESS_ARGS = ["-d", "-c", "--long=27", "-q"]
/** Fixed compression settings so the same commit packs to the same bytes. */
export const ZSTD_COMPRESS_ARGS = ["-19", "--long=27", "-T0", "-q", "-f"]

/** Parses and validates grammars.lock.json text. Throws on any bad field. */
export function parseLock(text: string): GrammarLock {
  let raw: unknown
  try {
    raw = JSON.parse(text)
  } catch (err) {
    throw new Error(`grammars lock is not valid JSON: ${errorMessage(err)}`)
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("grammars lock must be a JSON object")
  }
  const obj = raw as Record<string, unknown>
  const str = (key: string): string => {
    const value = obj[key]
    if (typeof value !== "string" || value.length === 0) {
      throw new Error(`grammars lock field "${key}" must be a non-empty string`)
    }
    return value
  }
  const url = str("url")
  if (!url.startsWith("https://")) {
    throw new Error(`grammars lock url must use https: ${url}`)
  }
  const sha256 = str("sha256")
  if (!SHA256_RE.test(sha256)) {
    throw new Error("grammars lock sha256 must be 64 lowercase hex characters")
  }
  const sizeBytes = obj.sizeBytes
  if (
    typeof sizeBytes !== "number" ||
    !Number.isSafeInteger(sizeBytes) ||
    sizeBytes <= 0
  ) {
    throw new Error("grammars lock sizeBytes must be a positive integer")
  }
  const sourceCommit = str("sourceCommit")
  if (!COMMIT_RE.test(sourceCommit)) {
    throw new Error(
      "grammars lock sourceCommit must be a full 40-character commit hash",
    )
  }
  const path = str("path")
  if (!isSafeRelativePath(path)) {
    throw new Error(`grammars lock path must be a safe relative path: ${path}`)
  }
  return {
    url,
    sha256,
    sizeBytes,
    sourceRepository: str("sourceRepository"),
    sourceCommit,
    path: path.replace(/\/+$/, ""),
  }
}

/**
 * True when `name` is a relative POSIX path with no empty, "." or ".."
 * components. A single trailing slash (tar directory entries) is allowed.
 */
export function isSafeRelativePath(name: string): boolean {
  if (name.length === 0 || name.includes("\0") || name.includes("\\")) {
    return false
  }
  if (name.startsWith("/") || /^[A-Za-z]:/.test(name)) return false
  const trimmed = name.endsWith("/") ? name.slice(0, -1) : name
  if (trimmed.length === 0) return false
  return trimmed
    .split("/")
    .every((part) => part !== "" && part !== "." && part !== "..")
}

/**
 * Checks archive member names: each must be safe and lie under `root`.
 * Returns the problems found; an empty list means the listing is acceptable.
 */
export function checkArchiveEntries(entries: string[], root: string): string[] {
  const problems: string[] = []
  const prefix = `${root}/`
  if (entries.length === 0) problems.push("archive is empty")
  for (const entry of entries) {
    if (!isSafeRelativePath(entry)) {
      problems.push(`unsafe archive path: ${JSON.stringify(entry)}`)
    } else if (entry.endsWith("/") && prefix.startsWith(entry)) {
      // git archive emits the root's parent directories as directory entries.
      continue
    } else if (entry !== root && entry !== prefix && !entry.startsWith(prefix)) {
      problems.push(`archive path outside ${root}/: ${JSON.stringify(entry)}`)
    }
  }
  return problems
}

/** sha256 of a file, streamed so large archives do not load into memory. */
export async function sha256File(path: string): Promise<string> {
  const hash = createHash("sha256")
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest("hex")
}

/** Throws unless the file has the expected size and sha256. */
export async function verifyArchive(
  path: string,
  lock: Pick<GrammarLock, "sha256" | "sizeBytes">,
): Promise<void> {
  const { size } = await lstat(path)
  if (size !== lock.sizeBytes) {
    throw new Error(
      `${path}: size ${size} bytes, grammars lock expects ${lock.sizeBytes}`,
    )
  }
  const actual = await sha256File(path)
  if (actual !== lock.sha256) {
    throw new Error(
      `${path}: sha256 ${actual}, grammars lock expects ${lock.sha256}`,
    )
  }
}

/** Default archive cache: ${XDG_CACHE_HOME:-~/.cache}/jiyuu-mori/grammars. */
export function defaultCacheDir(
  env: Record<string, string | undefined> = process.env,
  home: string = homedir(),
): string {
  const base = env.XDG_CACHE_HOME || join(home, ".cache")
  return join(base, "jiyuu-mori", "grammars")
}

export function cachedArchivePath(cacheDir: string, sha256: string): string {
  return join(cacheDir, `${sha256}.tar.zst`)
}

/** Size and mtime (ms) of an installed file, as recorded in the stamp. */
export type FileStat = [size: number, mtimeMs: number]

/**
 * Contents of the stamp file: the sha256 of the archive the grammars came
 * from and the size and mtime of every installed file, so a later run can
 * notice an edited, deleted or added file without rehashing 1.3 GB.
 */
export interface InstallStamp {
  archiveSha256: string
  files: Record<string, FileStat>
}

/** Parses the stamp; returns null for a missing, older or damaged stamp. */
export function parseStamp(text: string | null): InstallStamp | null {
  if (text === null) return null
  let raw: unknown
  try {
    raw = JSON.parse(text)
  } catch {
    return null
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null
  const { archiveSha256, files } = raw as Record<string, unknown>
  if (typeof archiveSha256 !== "string" || !SHA256_RE.test(archiveSha256)) {
    return null
  }
  if (!files || typeof files !== "object" || Array.isArray(files)) return null
  for (const value of Object.values(files)) {
    if (
      !Array.isArray(value) ||
      value.length !== 2 ||
      !value.every((n) => typeof n === "number" && Number.isFinite(n))
    ) {
      return null
    }
  }
  return { archiveSha256, files: files as Record<string, FileStat> }
}

export function formatStamp(stamp: InstallStamp): string {
  return `${JSON.stringify(stamp)}\n`
}

/** Size and mtime of every regular file under `dir`, plus anything else. */
export async function snapshotTree(
  dir: string,
): Promise<{ files: Record<string, FileStat>; other: string[] }> {
  const { files, other } = await walk(dir)
  const out: Record<string, FileStat> = {}
  for (const rel of files) {
    const info = await lstat(join(dir, rel))
    out[rel] = [info.size, info.mtimeMs]
  }
  return { files: out, other }
}

/**
 * Compares an installed tree's snapshot with the stamp and the expected
 * file list. Returns the problems found; an empty list means every expected
 * file is present with the size and mtime it had when it was verified, and
 * nothing else is there.
 */
export function compareSnapshot(
  stamp: InstallStamp,
  snapshot: { files: Record<string, FileStat>; other: string[] },
  expected: Map<string, string>,
): string[] {
  const problems: string[] = []
  for (const rel of snapshot.other) problems.push(`not a regular file: ${rel}`)
  for (const rel of expected.keys()) {
    const seen = snapshot.files[rel]
    const recorded = stamp.files[rel]
    if (seen === undefined) problems.push(`missing: ${rel}`)
    else if (
      recorded === undefined ||
      seen[0] !== recorded[0] ||
      seen[1] !== recorded[1]
    ) {
      problems.push(`changed since install: ${rel}`)
    }
  }
  for (const rel of Object.keys(snapshot.files)) {
    if (!expected.has(rel)) problems.push(`not in vendored-checksums.txt: ${rel}`)
  }
  return problems
}

/**
 * The files hashed on every no-op run: MANIFEST.md and each grammar's
 * license files. They are small, so the spot check costs little.
 */
export function sampleFiles(expected: Map<string, string>): string[] {
  return [...expected.keys()].filter(
    (rel) => rel === "MANIFEST.md" || /^[^/]+\/LICENSE[^/]*$/.test(rel),
  )
}

export async function readOptional(path: string): Promise<string | null> {
  try {
    return await readFile(path, "utf8")
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return null
    throw err
  }
}

/**
 * Parses scripts/vendored-checksums.txt ("<sha256>  <path>" per line) and
 * keeps the entries under `prefix`, keyed by path relative to it.
 */
export function parseChecksums(
  text: string,
  prefix: string,
): Map<string, string> {
  const dir = `${prefix.replace(/\/+$/, "")}/`
  const out = new Map<string, string>()
  const lines = text.split("\n")
  lines.forEach((line, index) => {
    if (line.trim() === "" || line.startsWith("#")) return
    const match = /^([0-9a-f]{64}) {2}(\S.*)$/.exec(line)
    if (!match) {
      throw new Error(`checksum line ${index + 1} is malformed: ${line}`)
    }
    const [, sha, path] = match as unknown as [string, string, string]
    if (!path.startsWith(dir)) return
    const rel = path.slice(dir.length)
    if (out.has(rel)) throw new Error(`duplicate checksum entry: ${path}`)
    out.set(rel, sha)
  })
  return out
}

export interface TreeReport {
  missing: string[]
  mismatched: string[]
  unlisted: string[]
  notRegular: string[]
  checked: number
}

export function treeReportOk(report: TreeReport): boolean {
  return (
    report.missing.length === 0 &&
    report.mismatched.length === 0 &&
    report.unlisted.length === 0 &&
    report.notRegular.length === 0
  )
}

/** Lists every entry under `dir` as POSIX paths relative to it. */
export async function walk(
  dir: string,
): Promise<{ files: string[]; other: string[] }> {
  const files: string[] = []
  const other: string[] = []
  const visit = async (rel: string): Promise<void> => {
    const entries = await readdir(join(dir, rel), { withFileTypes: true })
    for (const entry of entries) {
      const child = rel ? posix.join(rel, entry.name) : entry.name
      if (entry.isDirectory()) await visit(child)
      else if (entry.isFile()) files.push(child)
      else other.push(child)
    }
  }
  await visit("")
  return { files: files.sort(), other: other.sort() }
}

/**
 * Compares the files under `dir` with the expected checksums: every listed
 * file must exist with the right hash, and no unlisted file may be present.
 */
export async function verifyTree(
  dir: string,
  expected: Map<string, string>,
): Promise<TreeReport> {
  const { files, other } = await walk(dir)
  const present = new Set(files)
  const report: TreeReport = {
    missing: [],
    mismatched: [],
    unlisted: files.filter((file) => !expected.has(file)),
    notRegular: other,
    checked: 0,
  }
  for (const [rel, sha] of expected) {
    if (!present.has(rel)) {
      report.missing.push(rel)
      continue
    }
    report.checked++
    if ((await sha256File(join(dir, rel))) !== sha) report.mismatched.push(rel)
  }
  return report
}

export function formatTreeReport(report: TreeReport, limit = 10): string {
  const parts: string[] = []
  const add = (label: string, items: string[]): void => {
    if (items.length === 0) return
    const shown = items.slice(0, limit).join(", ")
    const more = items.length > limit ? `, and ${items.length - limit} more` : ""
    parts.push(`${items.length} ${label}: ${shown}${more}`)
  }
  add("missing", report.missing)
  add("with a wrong sha256", report.mismatched)
  add("not in vendored-checksums.txt", report.unlisted)
  add("not regular files", report.notRegular)
  return parts.join("; ")
}

/** Runs a command and resolves with stdout; rejects with stderr on failure. */
export function runCommand(
  command: string,
  args: string[],
  options: { cwd?: string } = {},
): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      stdio: ["ignore", "pipe", "pipe"],
    })
    const out: Buffer[] = []
    const err: Buffer[] = []
    child.stdout.on("data", (chunk: Buffer) => out.push(chunk))
    child.stderr.on("data", (chunk: Buffer) => err.push(chunk))
    child.on("error", (error) =>
      reject(new Error(`cannot run ${command}: ${error.message}`)),
    )
    child.on("close", (code) => {
      if (code === 0) resolve(Buffer.concat(out).toString("utf8"))
      else {
        const detail = Buffer.concat(err).toString("utf8").trim()
        reject(
          new Error(
            `${command} ${args.join(" ")} exited with ${code}` +
              (detail ? `: ${detail}` : ""),
          ),
        )
      }
    })
  })
}

/**
 * Runs `producer | consumer` without a shell. Both must exit 0. The consumer's
 * stdout is collected and returned.
 */
export function runPipeline(
  producer: [string, string[]],
  consumer: [string, string[]],
  options: { cwd?: string } = {},
): Promise<string> {
  return new Promise((resolve, reject) => {
    const left = spawn(producer[0], producer[1], {
      cwd: options.cwd,
      stdio: ["ignore", "pipe", "pipe"],
    })
    const right = spawn(consumer[0], consumer[1], {
      cwd: options.cwd,
      stdio: ["pipe", "pipe", "pipe"],
    })
    left.stdout.pipe(right.stdin)
    const out: Buffer[] = []
    const errs: Record<string, Buffer[]> = { left: [], right: [] }
    left.stderr.on("data", (c: Buffer) => errs.left!.push(c))
    right.stderr.on("data", (c: Buffer) => errs.right!.push(c))
    right.stdout.on("data", (c: Buffer) => out.push(c))
    right.stdin.on("error", () => {
      /* Reported through the consumer's exit status. */
    })
    let pending = 2
    let failure: Error | null = null
    const done = (
      name: string,
      side: "left" | "right",
      code: number | null,
    ): void => {
      if (code !== 0 && !failure) {
        const detail = Buffer.concat(errs[side]!).toString("utf8").trim()
        failure = new Error(
          `${name} exited with ${code}` + (detail ? `: ${detail}` : ""),
        )
      }
      if (--pending === 0) {
        if (failure) reject(failure)
        else resolve(Buffer.concat(out).toString("utf8"))
      }
    }
    left.on("error", (e) => reject(new Error(`cannot run ${producer[0]}: ${e.message}`)))
    right.on("error", (e) => reject(new Error(`cannot run ${consumer[0]}: ${e.message}`)))
    left.on("close", (code) => done(producer[0], "left", code))
    right.on("close", (code) => done(consumer[0], "right", code))
  })
}

/** One archive member: its name and the tar -tv type character. */
export interface ArchiveMember {
  name: string
  /** "-" regular file, "d" directory, "l" symlink, "h" hard link, ... */
  type: string
}

/**
 * Pairs the `tar -tf` names with the type column of `tar -tvf`. The two
 * listings must have the same length; a name with a newline in it would
 * split differently, so a mismatch is refused rather than guessed at.
 */
export function pairListings(
  names: string[],
  verbose: string[],
): ArchiveMember[] {
  if (names.length !== verbose.length) {
    throw new Error(
      `archive listings disagree: ${names.length} names, ` +
        `${verbose.length} verbose lines`,
    )
  }
  return names.map((name, i) => ({ name, type: verbose[i]!.charAt(0) }))
}

/**
 * Checks member types: only regular files and directories may be unpacked.
 * Symlinks, hard links and device or FIFO entries are refused before
 * extraction, so the result never depends on tar's link handling.
 */
export function checkMemberTypes(members: ArchiveMember[]): string[] {
  const problems: string[] = []
  for (const { name, type } of members) {
    if (type === "-" || type === "d") continue
    const kind =
      type === "l" ? "symlink" : type === "h" ? "hard link" : `type ${JSON.stringify(type)}`
    problems.push(`${kind} member refused: ${JSON.stringify(name)}`)
  }
  return problems
}

/** Members of a .tar.zst archive, with their types. */
export async function listArchive(archive: string): Promise<ArchiveMember[]> {
  const list = async (flags: string): Promise<string[]> =>
    (
      await runPipeline(
        ["zstd", [...ZSTD_DECOMPRESS_ARGS, archive]],
        ["tar", [flags, "-"]],
      )
    )
      .split("\n")
      .filter((line) => line.length > 0)
  return pairListings(await list("-tf"), await list("-tvf"))
}

/** Extracts a .tar.zst archive into an existing directory. */
export async function extractArchive(
  archive: string,
  dest: string,
): Promise<void> {
  await runPipeline(
    ["zstd", [...ZSTD_DECOMPRESS_ARGS, archive]],
    ["tar", ["-xf", "-", "-C", dest, "--no-same-owner"]],
  )
}

export function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}
