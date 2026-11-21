#!/usr/bin/env bun
// Release helper for Mori's C binary (codebase-memory-mcp).
//
// Usage (from mori/):
//   bun scripts/release.ts validate [mori/vX.Y.Z]
//   bun scripts/release.ts asset-name <version> <target>
//   bun scripts/release.ts prepare <version> <dir> [--tag T] [--commit SHA]
//   bun scripts/release.ts publish <mori/vX.Y.Z> <dir> --repo OWNER/NAME
//
// `validate` checks that a tag matches the VERSION file and prints the
// version; with no tag it checks the file alone. `prepare` writes
// checksums.txt, release-metadata.json and release-notes.md beside the
// archives. `publish` creates or refills a DRAFT GitHub release, never marked
// latest, and refuses to touch a release that is already published.
//
// Targets are <os>-<arch> as mise's github: backend reads them: static Linux
// x64 and arm64 builds and a dynamic, ad-hoc signed macOS arm64 build.

import { readdir, readFile, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"

export const BINARY = "codebase-memory-mcp"
export const TAG_PREFIX = "mori/v"
export const TARGETS = ["linux-x64", "linux-arm64", "darwin-arm64"] as const
export type Target = (typeof TARGETS)[number]

const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?$/

export interface ReleaseMetadata {
  name: string
  binary: string
  version: string
  tag: string
  commit: string
  assets: Record<string, string>
  targets: string[]
}

export interface GhRelease {
  id: number
  tag_name: string
  draft: boolean
}

export interface CommandResult {
  exitCode: number
  stdout: string
  stderr: string
}

export type Runner = (argv: string[]) => CommandResult

export function parseVersion(text: string): string {
  const version = text.trim()
  if (!SEMVER.test(version)) {
    throw new Error(`VERSION must be X.Y.Z or X.Y.Z-pre, got '${version}'`)
  }
  return version
}

export function tagFor(version: string): string {
  return `${TAG_PREFIX}${parseVersion(version)}`
}

export function versionFromTag(tag: string): string {
  if (!tag.startsWith(TAG_PREFIX)) {
    throw new Error(`Tag must be ${TAG_PREFIX}X.Y.Z, got '${tag}'`)
  }
  return parseVersion(tag.slice(TAG_PREFIX.length))
}

export async function readVersionFile(root: string): Promise<string> {
  return parseVersion(await readFile(join(root, "VERSION"), "utf8"))
}

// The tag's version must equal mori/VERSION, the version build.sh stamps
// into the binary (`codebase-memory-mcp --version`).
export function checkTag(tag: string, fileVersion: string): string {
  const version = versionFromTag(tag)
  if (version !== fileVersion) {
    throw new Error(
      `Tag ${tag} does not match mori/VERSION ${fileVersion}; ` +
        "bump VERSION in a PR before tagging",
    )
  }
  return version
}

export function assetName(version: string, target: string): string {
  if (!(TARGETS as readonly string[]).includes(target)) {
    throw new Error(
      `Unsupported target '${target}'; expected ${TARGETS.join(", ")}`,
    )
  }
  return `${BINARY}-${parseVersion(version)}-${target}.tar.gz`
}

export function releaseAssets(version: string): string[] {
  return TARGETS.map((target) => assetName(version, target))
}

export async function sha256File(path: string): Promise<string> {
  const hasher = new Bun.CryptoHasher("sha256")
  hasher.update(await Bun.file(path).arrayBuffer())
  return hasher.digest("hex")
}

export function formatChecksums(assets: Record<string, string>): string {
  return Object.keys(assets)
    .sort()
    .map((name) => `${assets[name]}  ${name}\n`)
    .join("")
}

export function releaseNotes(meta: ReleaseMetadata): string {
  const rows = Object.keys(meta.assets)
    .sort()
    .map((name) => `| \`${name}\` | \`${meta.assets[name]}\` |`)
  return [
    `# Mori ${meta.version}`,
    "",
    `Binary \`${meta.binary}\` built from Jiyuu commit \`${meta.commit}\`.`,
    "Static Linux builds for x64 and arm64, and a macOS arm64 (Apple",
    "silicon) build for macOS 14 or later. Each archive holds the binary,",
    "LICENSE, NOTICE and THIRD_PARTY_NOTICES.md.",
    "",
    "The macOS binary is ad-hoc signed but not notarized. If macOS blocks it",
    "after a browser download, remove the quarantine attribute:",
    "`xattr -d com.apple.quarantine codebase-memory-mcp`.",
    "",
    "| Asset | SHA-256 |",
    "| --- | --- |",
    ...rows,
    "",
  ].join("\n")
}

export async function prepare(
  dir: string,
  version: string,
  tag: string,
  commit: string,
): Promise<ReleaseMetadata> {
  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error(`Invalid source commit '${commit}'`)
  }
  const expected = releaseAssets(version)
  const present = new Set(await readdir(dir))
  const missing = expected.filter((name) => !present.has(name))
  if (missing.length > 0) {
    throw new Error(`Missing release archives: ${missing.join(", ")}`)
  }
  const assets: Record<string, string> = {}
  for (const name of expected) assets[name] = await sha256File(join(dir, name))
  const meta: ReleaseMetadata = {
    name: "mori",
    binary: BINARY,
    version,
    tag,
    commit,
    assets,
    targets: [...TARGETS],
  }
  await writeFile(join(dir, "checksums.txt"), formatChecksums(assets))
  await writeFile(
    join(dir, "release-metadata.json"),
    JSON.stringify(meta, null, 2) + "\n",
  )
  await writeFile(join(dir, "release-notes.md"), releaseNotes(meta))
  return meta
}

export function publishFiles(version: string): string[] {
  return [...releaseAssets(version), "checksums.txt", "release-metadata.json"]
}

export function requireUnpublished(release?: GhRelease): void {
  if (release && !release.draft) {
    throw new Error(
      `Release ${release.tag_name} is already published; refusing to change it`,
    )
  }
}

function runChecked(run: Runner, argv: string[]): string {
  const result = run(argv)
  if (result.exitCode !== 0) {
    throw new Error(
      `${argv.slice(0, 3).join(" ")} failed (${result.exitCode}): ` +
        result.stderr.trim(),
    )
  }
  return result.stdout
}

export const spawnRunner: Runner = (argv) => {
  const proc = Bun.spawnSync(argv, { stdout: "pipe", stderr: "pipe" })
  return {
    exitCode: proc.exitCode ?? 1,
    stdout: proc.stdout.toString(),
    stderr: proc.stderr.toString(),
  }
}

// Creates the draft on the first run and re-uploads with --clobber on a
// rerun. Drafts are only visible in the release list, not by tag lookup.
export function publish(
  dir: string,
  tag: string,
  repo: string,
  run: Runner = spawnRunner,
): void {
  const version = versionFromTag(tag)
  const listReleases = (): GhRelease[] =>
    (
      JSON.parse(
        runChecked(run, [
          "gh",
          "api",
          `repos/${repo}/releases`,
          "--paginate",
          "--slurp",
        ]),
      ) as GhRelease[][]
    ).flat()
  const existing = listReleases().find((r) => r.tag_name === tag)
  requireUnpublished(existing)
  const files = publishFiles(version).map((name) => join(dir, name))
  if (existing) {
    runChecked(run, [
      "gh",
      "release",
      "upload",
      tag,
      "--repo",
      repo,
      "--clobber",
      ...files,
    ])
    // Rebuilt archives get new SHA-256s; keep the notes in step with them.
    runChecked(run, [
      "gh",
      "release",
      "edit",
      tag,
      "--repo",
      repo,
      "--notes-file",
      join(dir, "release-notes.md"),
    ])
    return
  }
  runChecked(run, [
    "gh",
    "release",
    "create",
    tag,
    "--repo",
    repo,
    "--verify-tag",
    "--draft",
    "--latest=false",
    ...(version.includes("-") ? ["--prerelease"] : []),
    "--title",
    `Mori ${version}`,
    "--notes-file",
    join(dir, "release-notes.md"),
    ...files,
  ])
}

function flag(args: string[], name: string): string | undefined {
  const index = args.indexOf(name)
  if (index === -1) return undefined
  const value = args[index + 1]
  if (value === undefined || value.startsWith("--")) {
    throw new Error(`${name} needs a value`)
  }
  args.splice(index, 2)
  return value
}

export async function main(argv: string[], root: string): Promise<string> {
  const args = [...argv]
  const command = args.shift()
  if (command === "validate") {
    const fileVersion = await readVersionFile(root)
    const tag = args[0]
    return tag ? checkTag(tag, fileVersion) : fileVersion
  }
  if (command === "asset-name") {
    const [version, target] = args
    if (!version || !target) throw new Error("asset-name <version> <target>")
    return assetName(version, target)
  }
  if (command === "prepare") {
    const tagArg = flag(args, "--tag")
    const commitArg = flag(args, "--commit")
    const [versionArg, dir] = args
    if (!versionArg || !dir) throw new Error("prepare <version> <dir>")
    const version = parseVersion(versionArg)
    const commit =
      commitArg ??
      runChecked(spawnRunner, ["git", "rev-parse", "HEAD"]).trim()
    await prepare(resolve(dir), version, tagArg ?? tagFor(version), commit)
    return `prepared ${dir}`
  }
  if (command === "publish") {
    const repo = flag(args, "--repo")
    const [tag, dir] = args
    if (!tag || !dir || !repo) {
      throw new Error("publish <tag> <dir> --repo OWNER/NAME")
    }
    checkTag(tag, await readVersionFile(root))
    publish(resolve(dir), tag, repo)
    return `draft release ${tag} ready in ${repo}`
  }
  throw new Error("Expected validate, asset-name, prepare or publish")
}

if (import.meta.main) {
  const root = resolve(import.meta.dir, "..")
  main(process.argv.slice(2), root)
    .then((out) => console.log(out))
    .catch((error: unknown) => {
      console.error(
        `release: ${error instanceof Error ? error.message : String(error)}`,
      )
      process.exitCode = 1
    })
}
