import { afterEach, describe, expect, test } from "bun:test"
import { createHash } from "node:crypto"
import { link, mkdir, mkdtemp, readFile, rm, stat, symlink, utimes, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { fetchGrammars, parseFetchArgs, quickCheck, STAMP_NAME } from "./grammars-fetch.ts"
import {
  cachedArchivePath,
  checkArchiveEntries,
  checkMemberTypes,
  compareSnapshot,
  defaultCacheDir,
  formatStamp,
  type InstallStamp,
  isSafeRelativePath,
  listArchive,
  pairListings,
  parseChecksums,
  parseLock,
  parseStamp,
  runCommand,
  sampleFiles,
  sha256File,
  verifyArchive,
  verifyTree,
  ZSTD_COMPRESS_ARGS,
} from "./grammars-lib.ts"
import { checkGrammarListing, isInside, parsePackArgs, releaseUrl, updatedLock } from "./grammars-pack.ts"

const GRAMMARS = "internal/cbm/vendored/grammars"
const SHA_A = "a".repeat(64)
const COMMIT = "c".repeat(40)

const temps: string[] = []
async function tempDir(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "mori-grammars-test-"))
  temps.push(dir)
  return dir
}
afterEach(async () => {
  for (const dir of temps.splice(0)) await rm(dir, { recursive: true, force: true })
})

const sha = (text: string) => createHash("sha256").update(text).digest("hex")

/** Awaits a promise that must reject and returns its error message. */
async function rejection(promise: Promise<unknown>): Promise<string> {
  try {
    await promise
  } catch (err) {
    return err instanceof Error ? err.message : String(err)
  }
  throw new Error("expected the promise to reject")
}

function lockJson(overrides: Record<string, unknown> = {}): string {
  return JSON.stringify({
    url: "https://example.invalid/grammars.tar.zst",
    sha256: SHA_A,
    sizeBytes: 10,
    sourceRepository: "https://github.com/Unique-Divine/codebase-memory-mcp",
    sourceCommit: COMMIT,
    path: GRAMMARS,
    ...overrides,
  })
}

const FILES: Record<string, string> = {
  "MANIFEST.md": "- Grammars: **2**\n",
  "alpha/LICENSE": "MIT alpha\n",
  "alpha/parser.c": "int alpha;\n",
  "beta/LICENSE": "MIT beta\n",
  "beta/parser.c": "int beta;\n",
}

/** Writes FILES under <dir>/<GRAMMARS> and packs them as a .tar.zst. */
async function buildArchive(
  dir: string,
  files: Record<string, string> = FILES,
  members: string[] = [GRAMMARS],
): Promise<string> {
  const src = join(dir, "src")
  for (const [rel, body] of Object.entries(files)) {
    const path = join(src, GRAMMARS, rel)
    await mkdir(join(path, ".."), { recursive: true })
    await writeFile(path, body)
  }
  const tar = join(dir, "grammars.tar")
  await runCommand("tar", ["-cf", tar, "-C", src, ...members])
  const out = join(dir, "grammars.tar.zst")
  await runCommand("zstd", [...ZSTD_COMPRESS_ARGS, tar, "-o", out])
  return out
}

/** A fake mori root whose lock and checksums describe `archive`. */
async function buildRoot(
  dir: string,
  archive: string,
  checksumFiles: Record<string, string> = FILES,
): Promise<string> {
  const root = join(dir, "mori")
  await mkdir(join(root, "scripts"), { recursive: true })
  const lines = [
    `${sha("x")}  internal/cbm/vendored/common/LICENSE`,
    ...Object.entries(checksumFiles).map(([rel, body]) => `${sha(body)}  ${GRAMMARS}/${rel}`),
  ]
  await writeFile(join(root, "scripts", "vendored-checksums.txt"), `${lines.join("\n")}\n`)
  await writeFile(
    join(root, "grammars.lock.json"),
    lockJson({ sha256: await sha256File(archive), sizeBytes: (await stat(archive)).size }),
  )
  return root
}

describe("parseLock", () => {
  test("accepts a valid lock and trims a trailing slash on path", () => {
    const lock = parseLock(lockJson({ path: `${GRAMMARS}/` }))
    expect(lock.path).toBe(GRAMMARS)
    expect(lock.sha256).toBe(SHA_A)
    expect(lock.sizeBytes).toBe(10)
  })
  test.each([
    ["invalid JSON", "{", /not valid JSON/],
    ["array", "[]", /JSON object/],
    ["http url", lockJson({ url: "http://x/y" }), /https/],
    ["short sha", lockJson({ sha256: "abc" }), /sha256/],
    ["uppercase sha", lockJson({ sha256: "A".repeat(64) }), /sha256/],
    ["zero size", lockJson({ sizeBytes: 0 }), /sizeBytes/],
    ["fractional size", lockJson({ sizeBytes: 1.5 }), /sizeBytes/],
    ["short commit", lockJson({ sourceCommit: "cdfdf264" }), /sourceCommit/],
    ["absolute path", lockJson({ path: "/etc" }), /safe relative/],
    ["parent path", lockJson({ path: "a/../b" }), /safe relative/],
    ["missing repository", lockJson({ sourceRepository: "" }), /sourceRepository/],
  ])("rejects %s", (_name, text, pattern) => {
    expect(() => parseLock(text)).toThrow(pattern)
  })
})

describe("path safety", () => {
  test.each([
    ["a/b.c", true],
    ["a/b/", true],
    ["/abs", false],
    ["C:/win", false],
    ["../up", false],
    ["a/../b", false],
    ["a/./b", false],
    ["a//b", false],
    ["a\\b", false],
    ["", false],
    ["/", false],
  ])("isSafeRelativePath(%p) is %p", (name, ok) => {
    expect(isSafeRelativePath(name)).toBe(ok)
  })

  test("checkArchiveEntries accepts members under the root", () => {
    expect(checkArchiveEntries([`${GRAMMARS}/`, `${GRAMMARS}/a/LICENSE`], GRAMMARS)).toEqual([])
  })
  test("checkArchiveEntries accepts parent directory entries but not parent files", () => {
    const entries = ["internal/", "internal/cbm/", "internal/cbm/vendored/", `${GRAMMARS}/x`]
    expect(checkArchiveEntries(entries, GRAMMARS)).toEqual([])
    expect(checkArchiveEntries(["internal/cbm"], GRAMMARS)).toHaveLength(1)
    expect(checkArchiveEntries(["internal/cbm/other/"], GRAMMARS)).toHaveLength(1)
  })
  test("checkArchiveEntries reports unsafe and foreign members", () => {
    const problems = checkArchiveEntries(
      [`${GRAMMARS}/../../../evil`, "/etc/passwd", "other/file", `${GRAMMARS}x/file`],
      GRAMMARS,
    )
    expect(problems).toHaveLength(4)
    expect(problems[0]).toContain("unsafe")
    expect(problems[2]).toContain("outside")
    expect(problems[3]).toContain("outside")
  })
  test("checkArchiveEntries rejects an empty archive", () => {
    expect(checkArchiveEntries([], GRAMMARS)).toEqual(["archive is empty"])
  })
  test("pairListings zips names with the verbose type column", () => {
    expect(pairListings(["a/", "a/b"], ["drwx a/", "-rw- a/b"])).toEqual([
      { name: "a/", type: "d" },
      { name: "a/b", type: "-" },
    ])
    expect(() => pairListings(["a"], ["-rw- a", "-rw- b"])).toThrow(/disagree/)
  })
  test("checkMemberTypes allows only files and directories", () => {
    const problems = checkMemberTypes([
      { name: "f", type: "-" },
      { name: "d/", type: "d" },
      { name: "s", type: "l" },
      { name: "h", type: "h" },
      { name: "p", type: "p" },
    ])
    expect(problems).toEqual([
      'symlink member refused: "s"',
      'hard link member refused: "h"',
      'type "p" member refused: "p"',
    ])
  })
})

describe("checksums and stamp", () => {
  test("parseChecksums keeps only entries under the prefix", () => {
    const text = [
      "# comment",
      `${SHA_A}  internal/cbm/vendored/common/LICENSE`,
      `${"b".repeat(64)}  ${GRAMMARS}/MANIFEST.md`,
      "",
      `${"c".repeat(64)}  ${GRAMMARS}/go/parser.c`,
    ].join("\n")
    const map = parseChecksums(text, GRAMMARS)
    expect([...map.keys()]).toEqual(["MANIFEST.md", "go/parser.c"])
    expect(map.get("go/parser.c")).toBe("c".repeat(64))
  })
  test("parseChecksums rejects malformed and duplicate lines", () => {
    expect(() => parseChecksums("nothex  a/b", GRAMMARS)).toThrow(/line 1/)
    const dup = `${SHA_A}  ${GRAMMARS}/x\n${SHA_A}  ${GRAMMARS}/x\n`
    expect(() => parseChecksums(dup, GRAMMARS)).toThrow(/duplicate/)
  })
  test("parseStamp reads a stamp and rejects older or damaged ones", () => {
    const stamp: InstallStamp = { archiveSha256: SHA_A, files: { "a/LICENSE": [3, 1000] } }
    expect(parseStamp(formatStamp(stamp))).toEqual(stamp)
    expect(parseStamp(null)).toBeNull()
    expect(parseStamp(`${SHA_A}\n`)).toBeNull()
    expect(parseStamp(JSON.stringify({ archiveSha256: "x", files: {} }))).toBeNull()
    expect(parseStamp(JSON.stringify({ archiveSha256: SHA_A, files: { a: [1] } }))).toBeNull()
  })
  test("compareSnapshot reports missing, changed, unlisted and odd entries", () => {
    const expected = new Map([
      ["a", SHA_A],
      ["b", SHA_A],
      ["c", SHA_A],
    ])
    const stamp: InstallStamp = { archiveSha256: SHA_A, files: { a: [1, 10], b: [2, 20], c: [3, 30] } }
    const same = { files: { a: [1, 10], b: [2, 20], c: [3, 30] } as InstallStamp["files"], other: [] }
    expect(compareSnapshot(stamp, same, expected)).toEqual([])
    const drift = {
      files: { a: [1, 11], b: [9, 20], x: [1, 1] } as InstallStamp["files"],
      other: ["link"],
    }
    expect(compareSnapshot(stamp, drift, expected)).toEqual([
      "not a regular file: link",
      "changed since install: a",
      "changed since install: b",
      "missing: c",
      "not in vendored-checksums.txt: x",
    ])
  })
  test("sampleFiles picks MANIFEST.md and per-grammar license files", () => {
    const expected = new Map(
      ["MANIFEST.md", "a/LICENSE", "a/LICENSE-codegraph", "a/parser.c", "a/sub/LICENSE"].map(
        (rel) => [rel, SHA_A] as [string, string],
      ),
    )
    expect(sampleFiles(expected)).toEqual(["MANIFEST.md", "a/LICENSE", "a/LICENSE-codegraph"])
  })
  test("defaultCacheDir honours XDG_CACHE_HOME", () => {
    expect(defaultCacheDir({ XDG_CACHE_HOME: "/x" }, "/home/u")).toBe("/x/jiyuu-mori/grammars")
    expect(defaultCacheDir({}, "/home/u")).toBe("/home/u/.cache/jiyuu-mori/grammars")
    expect(cachedArchivePath("/c", SHA_A)).toBe(`/c/${SHA_A}.tar.zst`)
  })
})

describe("verifyArchive and verifyTree", () => {
  test("verifyArchive checks size then sha256", async () => {
    const dir = await tempDir()
    const file = join(dir, "f")
    await writeFile(file, "hello")
    await verifyArchive(file, { sha256: sha("hello"), sizeBytes: 5 })
    expect(await rejection(verifyArchive(file, { sha256: sha("hello"), sizeBytes: 6 }))).toMatch(/size 5/)
    expect(await rejection(verifyArchive(file, { sha256: SHA_A, sizeBytes: 5 }))).toMatch(/sha256/)
  })

  test("verifyTree reports missing, mismatched, unlisted and symlinked files", async () => {
    const dir = await tempDir()
    await mkdir(join(dir, "a"), { recursive: true })
    await writeFile(join(dir, "a", "ok"), "ok")
    await writeFile(join(dir, "a", "bad"), "changed")
    await writeFile(join(dir, "extra"), "x")
    await symlink("/etc/passwd", join(dir, "link"))
    const report = await verifyTree(
      dir,
      new Map([
        ["a/ok", sha("ok")],
        ["a/bad", sha("original")],
        ["gone", SHA_A],
      ]),
    )
    expect(report.missing).toEqual(["gone"])
    expect(report.mismatched).toEqual(["a/bad"])
    expect(report.unlisted).toEqual(["extra"])
    expect(report.notRegular).toEqual(["link"])
    expect(report.checked).toBe(2)
  })
})

describe("argument parsing", () => {
  test("fetch flags", () => {
    expect(parseFetchArgs([])).toEqual({ force: false })
    expect(parseFetchArgs(["--force", "--from", "/a.tar.zst"])).toEqual({
      force: true,
      from: "/a.tar.zst",
    })
    expect(() => parseFetchArgs(["--from"])).toThrow(/needs a value/)
    expect(() => parseFetchArgs(["--bogus"])).toThrow(/unknown/)
  })
  test("pack flags", () => {
    expect(() => parsePackArgs([])).toThrow(/--repo/)
    const args = parsePackArgs([
      "--repo", "/r", "--out", "/o.tar.zst", "--update-lock", "--release-tag", "mori/grammars/v2",
    ])
    expect(args).toEqual({
      repo: "/r",
      commit: undefined,
      out: "/o.tar.zst",
      updateLock: true,
      releaseTag: "mori/grammars/v2",
      url: undefined,
    })
    expect(parsePackArgs(["--repo", "/r", "--update-lock", "--url", "https://x/y"]).url).toBe("https://x/y")
    expect(() => parsePackArgs(["--repo", "/r", "--update-lock"])).toThrow(/--release-tag <tag> or --url/)
    expect(() => parsePackArgs(["--repo", "/r", "--release-tag", "t"])).toThrow(/only apply with --update-lock/)
    expect(() =>
      parsePackArgs(["--repo", "/r", "--update-lock", "--release-tag", "t", "--url", "https://x"]),
    ).toThrow(/not both/)
  })
  test("releaseUrl swaps the tag in a GitHub release URL", () => {
    const v1 = "https://github.com/Unique-Divine/jiyuu/releases/download/mori%2Fgrammars%2Fv1/grammars.tar.zst"
    expect(releaseUrl(v1, "mori/grammars/v2")).toBe(
      "https://github.com/Unique-Divine/jiyuu/releases/download/mori%2Fgrammars%2Fv2/grammars.tar.zst",
    )
    expect(() => releaseUrl("https://example.com/grammars.tar.zst", "v2")).toThrow(/pass --url/)
    expect(() => releaseUrl(v1, " ")).toThrow(/invalid release tag/)
  })
  test("updatedLock rewrites url, sha256, size and commit and validates them", () => {
    const raw = JSON.parse(lockJson()) as Record<string, unknown>
    const next = updatedLock(raw, {
      url: "https://example.invalid/v2/grammars.tar.zst",
      sha256: "b".repeat(64),
      sizeBytes: 42,
      commit: "d".repeat(40),
    })
    expect(parseLock(JSON.stringify(next))).toMatchObject({
      url: "https://example.invalid/v2/grammars.tar.zst",
      sha256: "b".repeat(64),
      sizeBytes: 42,
      sourceCommit: "d".repeat(40),
      path: GRAMMARS,
    })
    expect(() =>
      updatedLock(raw, { url: "http://insecure", sha256: SHA_A, sizeBytes: 1, commit: COMMIT }),
    ).toThrow(/https/)
  })
  test("isInside", () => {
    expect(isInside("/a/b/c", "/a/b")).toBe(true)
    expect(isInside("/a/b", "/a/b")).toBe(true)
    expect(isInside("/a/bc", "/a/b")).toBe(false)
    expect(isInside("/x", "/a/b")).toBe(false)
  })
  test("checkGrammarListing needs MANIFEST.md and a LICENSE per grammar", () => {
    const ok = Object.keys(FILES).map((rel) => `${GRAMMARS}/${rel}`)
    expect(checkGrammarListing(ok, GRAMMARS)).toEqual([])
    const bad = ok.filter((p) => !p.endsWith("MANIFEST.md") && p !== `${GRAMMARS}/beta/LICENSE`)
    expect(checkGrammarListing(bad, GRAMMARS)).toEqual([
      `${GRAMMARS}/MANIFEST.md is missing`,
      `${GRAMMARS}/beta/LICENSE is missing`,
    ])
  })
})

describe("fetchGrammars", () => {
  const quiet = () => {}
  const noDownload = async () => {
    throw new Error("unexpected download")
  }

  test("installs from --from, then skips, then reuses the cache", async () => {
    const dir = await tempDir()
    const archive = await buildArchive(dir)
    const root = await buildRoot(dir, archive)
    const cacheDir = join(dir, "cache")
    const base = { root, cacheDir, download: noDownload, log: quiet }

    expect(await fetchGrammars({ ...base, from: archive })).toBe("installed")
    expect(await readFile(join(root, GRAMMARS, "beta/parser.c"), "utf8")).toBe("int beta;\n")
    const lock = parseLock(await readFile(join(root, "grammars.lock.json"), "utf8"))
    expect(parseStamp(await readFile(join(root, STAMP_NAME), "utf8"))?.archiveSha256).toBe(lock.sha256)
    await stat(cachedArchivePath(cacheDir, lock.sha256))

    expect(await fetchGrammars(base)).toBe("up-to-date")

    // A forced reinstall without --from must come from the verified cache.
    await writeFile(join(root, GRAMMARS, "alpha/parser.c"), "tampered")
    expect(await fetchGrammars({ ...base, force: true })).toBe("installed")
    expect(await readFile(join(root, GRAMMARS, "alpha/parser.c"), "utf8")).toBe("int alpha;\n")
  })

  test("downloads when the cache is empty and replaces a corrupt cache entry", async () => {
    const dir = await tempDir()
    const archive = await buildArchive(dir)
    const root = await buildRoot(dir, archive)
    const cacheDir = join(dir, "cache")
    const lock = parseLock(await readFile(join(root, "grammars.lock.json"), "utf8"))
    await mkdir(cacheDir, { recursive: true })
    await writeFile(cachedArchivePath(cacheDir, lock.sha256), "corrupt")
    const urls: string[] = []
    const download = async (url: string, dest: string) => {
      urls.push(url)
      await writeFile(dest, await readFile(archive))
    }
    expect(await fetchGrammars({ root, cacheDir, download, log: quiet })).toBe("installed")
    expect(urls).toEqual([lock.url])
    expect(await sha256File(cachedArchivePath(cacheDir, lock.sha256))).toBe(lock.sha256)
  })

  test("refuses an archive whose sha256 differs from the lock", async () => {
    const dir = await tempDir()
    const archive = await buildArchive(dir)
    const root = await buildRoot(dir, archive)
    const other = join(dir, "other.tar.zst")
    await writeFile(other, Buffer.alloc((await stat(archive)).size, 1))
    expect(await rejection(
      fetchGrammars({ root, cacheDir: join(dir, "cache"), from: other, download: noDownload, log: quiet }),
    )).toMatch(/refusing to unpack: .*sha256/)
    expect(await rejection(stat(join(root, GRAMMARS)))).toBeString()
  })

  test("refuses an archive with members outside the grammar root", async () => {
    const dir = await tempDir()
    await mkdir(join(dir, "src", "stray"), { recursive: true })
    await writeFile(join(dir, "src", "stray", "x"), "x")
    const archive = await buildArchive(dir, FILES, [GRAMMARS, "stray"])
    const root = await buildRoot(dir, archive)
    expect(await rejection(
      fetchGrammars({ root, cacheDir: join(dir, "cache"), from: archive, download: noDownload, log: quiet }),
    )).toMatch(/outside/)
    expect(await rejection(stat(join(root, GRAMMARS)))).toBeString()
  })

  test("refuses unpacked files that disagree with vendored-checksums.txt", async () => {
    const dir = await tempDir()
    const archive = await buildArchive(dir)
    const root = await buildRoot(dir, archive, { ...FILES, "alpha/parser.c": "int other;\n" })
    expect(await rejection(
      fetchGrammars({ root, cacheDir: join(dir, "cache"), from: archive, download: noDownload, log: quiet }),
    )).toMatch(/wrong sha256: alpha\/parser.c/)
    expect(await rejection(stat(join(root, STAMP_NAME)))).toBeString()
  })

  test.each([
    ["symlink", (src: string) => symlink("/etc/passwd", join(src, GRAMMARS, "alpha/evil"))],
    ["hard link", (src: string) => link(join(src, GRAMMARS, "alpha/LICENSE"), join(src, GRAMMARS, "beta/COPY"))],
  ])("refuses a %s member before extracting anything", async (kind, addLink) => {
    const dir = await tempDir()
    const src = join(dir, "src")
    for (const [rel, body] of Object.entries(FILES)) {
      await mkdir(join(src, GRAMMARS, rel, ".."), { recursive: true })
      await writeFile(join(src, GRAMMARS, rel), body)
    }
    await addLink(src)
    const tar = join(dir, "grammars.tar")
    await runCommand("tar", ["-cf", tar, "-C", src, GRAMMARS])
    const archive = join(dir, "grammars.tar.zst")
    await runCommand("zstd", [...ZSTD_COMPRESS_ARGS, tar, "-o", archive])
    expect((await listArchive(archive)).some((m) => m.type === (kind === "symlink" ? "l" : "h"))).toBe(true)
    const root = await buildRoot(dir, archive)
    expect(await rejection(
      fetchGrammars({ root, cacheDir: join(dir, "cache"), from: archive, download: noDownload, log: quiet }),
    )).toMatch(new RegExp(`${kind} member refused`))
    expect(await rejection(stat(join(root, GRAMMARS)))).toBeString()
  })

  test("a no-op run notices edits, re-verifies, and reinstalls only on a real change", async () => {
    const dir = await tempDir()
    const archive = await buildArchive(dir)
    const root = await buildRoot(dir, archive)
    const base = { root, cacheDir: join(dir, "cache"), download: noDownload, log: quiet }
    expect(await fetchGrammars({ ...base, from: archive })).toBe("installed")
    const lock = parseLock(await readFile(join(root, "grammars.lock.json"), "utf8"))
    const expected = parseChecksums(
      await readFile(join(root, "scripts", "vendored-checksums.txt"), "utf8"),
      GRAMMARS,
    )
    const target = join(root, GRAMMARS)
    const check = async () =>
      quickCheck(target, await readFile(join(root, STAMP_NAME), "utf8"), lock.sha256, expected)
    expect(await check()).toEqual([])

    // Same size, new mtime: caught without hashing parser.c.
    await writeFile(join(target, "alpha/parser.c"), "int alphX;\n")
    expect(await check()).toEqual(["changed since install: alpha/parser.c"])
    expect(await fetchGrammars(base)).toBe("installed")
    expect(await readFile(join(target, "alpha/parser.c"), "utf8")).toBe("int alpha;\n")

    // A touched but unchanged file is re-verified and the stamp refreshed.
    const now = new Date()
    await utimes(join(target, "beta/parser.c"), now, now)
    expect(await fetchGrammars(base)).toBe("reverified")
    expect(await fetchGrammars(base)).toBe("up-to-date")

    // A license edited with its mtime restored is caught by the spot check.
    const licensePath = join(target, "beta/LICENSE")
    const before = await stat(licensePath)
    await writeFile(licensePath, "MIT betX\n")
    await utimes(licensePath, before.atime, before.mtime)
    expect(await check()).toEqual(["wrong sha256: beta/LICENSE"])

    // Deleted and added files are both caught.
    await rm(join(target, "alpha/parser.c"))
    await writeFile(join(target, "extra"), "x")
    expect(await check()).toEqual(["missing: alpha/parser.c", "not in vendored-checksums.txt: extra"])
    expect(await fetchGrammars(base)).toBe("installed")
    expect(await check()).toEqual([])
  })

  test("an older plain-sha256 stamp is upgraded after a full check", async () => {
    const dir = await tempDir()
    const archive = await buildArchive(dir)
    const root = await buildRoot(dir, archive)
    const base = { root, cacheDir: join(dir, "cache"), download: noDownload, log: quiet }
    expect(await fetchGrammars({ ...base, from: archive })).toBe("installed")
    const lock = parseLock(await readFile(join(root, "grammars.lock.json"), "utf8"))
    await writeFile(join(root, STAMP_NAME), `${lock.sha256}\n`)
    expect(await fetchGrammars(base)).toBe("reverified")
    expect(parseStamp(await readFile(join(root, STAMP_NAME), "utf8"))?.archiveSha256).toBe(lock.sha256)
    expect(await fetchGrammars(base)).toBe("up-to-date")
  })
})
