#!/usr/bin/env bun
// Release helper for Mori's binary `mori` (crate mori/jiyuu-mori with the C
// program linked in).
//
// Usage (from mori/):
//   bun scripts/release.ts validate [mori/vX.Y.Z]
//   bun scripts/release.ts plan <event> <ref> <repo> <dry-run> <prewarm> <daily-check>
//   bun scripts/release.ts asset-name <version> <target>
//   bun scripts/release.ts prepare <version> <dir> [--tag T] [--commit SHA]
//   bun scripts/release.ts prepare-preview <version> <dir> [--commit SHA]
//   bun scripts/release.ts publish <mori/vX.Y.Z> <dir> --repo OWNER/NAME
//
// `validate` checks that a tag matches the VERSION file and prints the
// version; with no tag it checks the file alone. `prepare` writes
// checksums.txt, release-metadata.json and release-notes.md beside the
// archives. `publish` creates or refills a DRAFT GitHub release, never marked
// latest, and refuses to touch a release that is already published.
//
// Targets are <os>-<arch> as mise's github: backend reads them: static Linux
// x64 and arm64 builds and dynamic, ad-hoc signed macOS arm64 and x64 builds.

import { readdir, readFile, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"

export const BINARY = "mori"
export const TAG_PREFIX = "mori/v"
const PLATFORMS = [
  {
    target: "linux-x64",
    goos: "linux",
    goarch: "amd64",
    rust_target: "x86_64-unknown-linux-gnu",
    runner: "ubuntu-latest",
    c_reference_sha256: "ba3cbc562e6c41585144e78857a54a51badae99bdb5e6050cedc79089817ed07",
  },
  {
    target: "linux-arm64",
    goos: "linux",
    goarch: "arm64",
    rust_target: "aarch64-unknown-linux-gnu",
    runner: "ubuntu-24.04-arm",
    c_reference_sha256: "a4984606e39dcdf6e5d4a86f31ceea4d0f49761fe4dc3eb91544cdc86e26e1bb",
  },
  {
    target: "darwin-arm64",
    goos: "darwin",
    goarch: "arm64",
    arch: "arm64",
    rust_target: "aarch64-apple-darwin",
    runner: "macos-15",
    c_reference_sha256: "3fb0ebdf5416fbabfb81e48754275b5f01088faac3dc5f015cbe6a84d743e837",
  },
  {
    target: "darwin-x64",
    goos: "darwin",
    goarch: "amd64",
    arch: "x86_64",
    rust_target: "x86_64-apple-darwin",
    runner: "macos-15-intel",
    c_reference_sha256: "",
  },
] as const
export type Target = (typeof PLATFORMS)[number]["target"]
export const TARGETS: readonly Target[] = PLATFORMS.map(({ target }) => target)
const PREVIEW_PLATFORMS = PLATFORMS.filter(({ target }) => target !== "darwin-x64")
const PREVIEW_TARGETS = PREVIEW_PLATFORMS.map(({ target }) => target)
type ReleaseMode = "daily" | "prewarm" | "preview" | "tag"

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

// The tag's version must equal mori/VERSION, the version the release build
// stamps into the binary (`mori --version`).
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

interface ReleaseRequest {
  event: string
  ref: string
  repo: string
  dryRun: boolean
  prewarm: boolean
  dailyCheck: boolean
}

export function releasePlan(request: ReleaseRequest, version: string) {
  const { event, ref, repo, dryRun, prewarm, dailyCheck } = request
  const tagRef = "refs/tags/"
  const tag = ref.startsWith(tagRef + TAG_PREFIX) ? ref.slice(tagRef.length) : ""
  const trustedMain = repo === "Unique-Divine/jiyuu" && ref === "refs/heads/main"
  let mode: ReleaseMode
  if (event === "schedule") {
    if (!trustedMain) {
      throw new Error("Scheduled package checks require main in Unique-Divine/jiyuu")
    }
    mode = "daily"
  } else if (event === "workflow_dispatch" && dailyCheck) {
    if (!trustedMain || !dryRun || prewarm) {
      throw new Error(
        "daily_check requires a dry run on main in Unique-Divine/jiyuu with prewarm_oracle=false",
      )
    }
    mode = "daily"
  } else if (event === "workflow_dispatch" && prewarm) {
    if (ref !== "refs/heads/main" || !dryRun) {
      throw new Error("prewarm_oracle requires a manual dry run on main")
    }
    mode = "prewarm"
  } else if (tag && (event === "push" || event === "workflow_dispatch")) {
    checkTag(tag, version)
    mode = "tag"
  } else if (event === "workflow_dispatch" && dryRun && ref.startsWith("refs/heads/")) {
    mode = "preview"
  } else {
    throw new Error("Expected a Mori tag or a branch dry run")
  }
  return {
    mode,
    version,
    tag: tagFor(version),
    publish: mode === "tag" && (event === "push" || !dryRun),
    matrix: {
      include: mode === "preview" || mode === "prewarm" ? PREVIEW_PLATFORMS : PLATFORMS,
    },
  }
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
    "",
    `[Install and upgrade instructions](https://github.com/Unique-Divine/jiyuu/blob/${meta.commit}/mori/README.md#install).`,
    "",
    meta.targets.includes("darwin-x64")
      ? "Static Linux builds for x64 and arm64, and macOS builds for Apple Silicon and Intel, for macOS 14 or later."
      : "Preview builds for Linux x64 and arm64 and for Apple Silicon on macOS 14 or later.",
    "Each archive",
    "holds the binary, LICENSE, NOTICE and THIRD_PARTY_NOTICES.md.",
    "",
    "The macOS binaries are ad-hoc signed but not notarized. If macOS blocks",
    "one after a browser download, remove the quarantine attribute:",
    `\`xattr -d com.apple.quarantine ${meta.binary}\`.`,
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
  preview = false,
): Promise<ReleaseMetadata> {
  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error(`Invalid source commit '${commit}'`)
  }
  const targets = preview ? PREVIEW_TARGETS : TARGETS
  const expected = targets.map((target) => assetName(version, target))
  const present = new Set(await readdir(dir))
  const missing = expected.filter((name) => !present.has(name))
  if (missing.length > 0) {
    throw new Error(`Missing release archives: ${missing.join(", ")}`)
  }
  const unexpected = [...present].filter(
    (name) => name.endsWith(".tar.gz") && !expected.includes(name),
  )
  if (unexpected.length > 0) {
    throw new Error(`Unexpected package archives: ${unexpected.sort().join(", ")}`)
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
    targets: [...targets],
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
  if (command === "plan") {
    const [event, ref, repo, dryRun, prewarm, dailyCheck] = args
    if (args.length !== 6 || !event || !ref || !repo) {
      throw new Error("plan <event> <ref> <repo> <dry-run> <prewarm> <daily-check>")
    }
    return JSON.stringify(
      releasePlan(
        {
          event,
          ref,
          repo,
          dryRun: dryRun === "true",
          prewarm: prewarm === "true",
          dailyCheck: dailyCheck === "true",
        },
        await readVersionFile(root),
      ),
    )
  }
  if (command === "asset-name") {
    const [version, target] = args
    if (!version || !target) throw new Error("asset-name <version> <target>")
    return assetName(version, target)
  }
  if (command === "prepare" || command === "prepare-preview") {
    const tagArg = flag(args, "--tag")
    const commitArg = flag(args, "--commit")
    const [versionArg, dir] = args
    if (!versionArg || !dir) throw new Error("prepare <version> <dir>")
    const version = parseVersion(versionArg)
    const commit =
      commitArg ??
      runChecked(spawnRunner, ["git", "rev-parse", "HEAD"]).trim()
    await prepare(resolve(dir), version, tagArg ?? tagFor(version), commit, command === "prepare-preview")
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
  throw new Error("Expected validate, plan, asset-name, prepare, prepare-preview or publish")
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
