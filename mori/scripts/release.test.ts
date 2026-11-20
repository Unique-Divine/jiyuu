import { describe, expect, test } from "bun:test"
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import {
  assetName,
  checkTag,
  formatChecksums,
  type GhRelease,
  main,
  parseVersion,
  prepare,
  publish,
  releaseAssets,
  type Runner,
  sha256File,
  versionFromTag,
} from "./release"

const COMMIT = "a".repeat(40)

async function withDir<T>(fn: (dir: string) => Promise<T>): Promise<T> {
  const dir = await mkdtemp(join(tmpdir(), "mori-release-"))
  try {
    return await fn(dir)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
}

function fakeGh(releases: GhRelease[]): { run: Runner; calls: string[][] } {
  const calls: string[][] = []
  const run: Runner = (argv) => {
    calls.push(argv)
    if (argv[1] === "api") {
      return { exitCode: 0, stdout: JSON.stringify([releases]), stderr: "" }
    }
    return { exitCode: 0, stdout: "", stderr: "" }
  }
  return { run, calls }
}

describe("versions and tags", () => {
  test("accepts semver and rejects junk", () => {
    expect(parseVersion("0.13.0\n")).toBe("0.13.0")
    expect(parseVersion("0.13.0-rc.1")).toBe("0.13.0-rc.1")
    expect(() => parseVersion("v0.13.0")).toThrow()
    expect(() => parseVersion("0.13")).toThrow()
    expect(() => parseVersion("dev")).toThrow()
  })

  test("tag must use the mori/v prefix", () => {
    expect(versionFromTag("mori/v0.13.0")).toBe("0.13.0")
    expect(() => versionFromTag("v0.13.0")).toThrow("mori/v")
    expect(() => versionFromTag("mori/grammars/v1")).toThrow()
  })

  test("tag must match VERSION", () => {
    expect(checkTag("mori/v0.13.0", "0.13.0")).toBe("0.13.0")
    expect(() => checkTag("mori/v0.13.1", "0.13.0")).toThrow("does not match")
  })

  test("validate reads the VERSION file", async () => {
    await withDir(async (dir) => {
      await writeFile(join(dir, "VERSION"), "0.13.0\n")
      expect(await main(["validate"], dir)).toBe("0.13.0")
      expect(await main(["validate", "mori/v0.13.0"], dir)).toBe("0.13.0")
      await expect(main(["validate", "mori/v0.14.0"], dir)).rejects.toThrow()
    })
  })

  test("repository VERSION is valid", async () => {
    const root = join(import.meta.dir, "..")
    expect(await main(["validate"], root)).toMatch(/^\d+\.\d+\.\d+/)
  })
})

describe("asset names", () => {
  test("use mise-friendly platform names", () => {
    expect(assetName("0.13.0", "x64")).toBe(
      "codebase-memory-mcp-0.13.0-linux-x64.tar.gz",
    )
    expect(releaseAssets("0.13.0")).toEqual([
      "codebase-memory-mcp-0.13.0-linux-x64.tar.gz",
      "codebase-memory-mcp-0.13.0-linux-arm64.tar.gz",
    ])
    expect(() => assetName("0.13.0", "amd64")).toThrow("Unsupported arch")
  })
})

describe("prepare", () => {
  test("writes checksums, metadata and notes", async () => {
    await withDir(async (dir) => {
      for (const name of releaseAssets("0.13.0")) {
        await writeFile(join(dir, name), `archive ${name}`)
      }
      const meta = await prepare(dir, "0.13.0", "mori/v0.13.0", COMMIT)
      const checksums = await readFile(join(dir, "checksums.txt"), "utf8")
      expect(checksums).toBe(formatChecksums(meta.assets))
      const x64 = "codebase-memory-mcp-0.13.0-linux-x64.tar.gz"
      expect(checksums).toContain(
        `${await sha256File(join(dir, x64))}  ${x64}\n`,
      )
      const written = JSON.parse(
        await readFile(join(dir, "release-metadata.json"), "utf8"),
      )
      expect(written.tag).toBe("mori/v0.13.0")
      expect(written.targets).toEqual(["linux-x64", "linux-arm64"])
      const notes = await readFile(join(dir, "release-notes.md"), "utf8")
      expect(notes).toContain(COMMIT)
    })
  })

  test("fails when an archive is missing", async () => {
    await withDir(async (dir) => {
      await writeFile(
        join(dir, "codebase-memory-mcp-0.13.0-linux-x64.tar.gz"),
        "x",
      )
      await expect(
        prepare(dir, "0.13.0", "mori/v0.13.0", COMMIT),
      ).rejects.toThrow("linux-arm64")
    })
  })
})

describe("publish", () => {
  test("creates a draft that is not marked latest", () => {
    const { run, calls } = fakeGh([])
    publish("/rel", "mori/v0.13.0", "o/r", run)
    const create = calls.find((argv) => argv[2] === "create")!
    expect(create).toContain("--draft")
    expect(create).toContain("--latest=false")
    expect(create).toContain("--verify-tag")
    expect(create).toContain("/rel/checksums.txt")
  })

  test("refills an existing draft", () => {
    const { run, calls } = fakeGh([
      { id: 1, tag_name: "mori/v0.13.0", draft: true },
    ])
    publish("/rel", "mori/v0.13.0", "o/r", run)
    const upload = calls.find((argv) => argv[2] === "upload")!
    expect(upload).toContain("--clobber")
    expect(calls.some((argv) => argv[2] === "create")).toBe(false)
  })

  test("refuses a published release", () => {
    const { run, calls } = fakeGh([
      { id: 1, tag_name: "mori/v0.13.0", draft: false },
    ])
    expect(() => publish("/rel", "mori/v0.13.0", "o/r", run)).toThrow(
      "already published",
    )
    expect(calls.length).toBe(1)
  })
})
