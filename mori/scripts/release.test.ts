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
  releasePlan,
  type Runner,
  sha256File,
  type Target,
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
    expect(assetName("0.13.0", "linux-x64")).toBe(
      "mori-0.13.0-linux-x64.tar.gz",
    )
    expect(assetName("0.13.0", "darwin-arm64")).toBe(
      "mori-0.13.0-darwin-arm64.tar.gz",
    )
    expect(releaseAssets("0.13.0")).toEqual([
      "mori-0.13.0-linux-x64.tar.gz",
      "mori-0.13.0-linux-arm64.tar.gz",
      "mori-0.13.0-darwin-arm64.tar.gz",
      "mori-0.13.0-darwin-x64.tar.gz",
    ])
  })

  test("reject targets the workflow does not build", () => {
    expect(() => assetName("0.13.0", "x64")).toThrow("Unsupported target")
    expect(() => assetName("0.13.0", "linux-amd64")).toThrow()
    expect(() => assetName("0.13.0", "darwin-amd64")).toThrow()
    expect(() => assetName("0.13.0", "windows-x64")).toThrow()
  })

  test("asset-name command prints the archive name", async () => {
    expect(await main(["asset-name", "0.13.0", "darwin-arm64"], "/")).toBe(
      "mori-0.13.0-darwin-arm64.tar.gz",
    )
  })
})

describe("workflow modes", () => {
  const mainDispatch = {
    event: "workflow_dispatch",
    ref: "refs/heads/main",
    repo: "Unique-Divine/jiyuu",
    dryRun: true,
    prewarm: false,
    dailyCheck: false,
  }
  const tag = "refs/tags/mori/v0.13.0"
  const previewTargets: Target[] = ["linux-x64", "linux-arm64", "darwin-arm64"]
  const allTargets: Target[] = [...previewTargets, "darwin-x64"]

  test.each([
    ["preview", { ref: "refs/heads/feature" }, false, previewTargets],
    ["prewarm", { prewarm: true }, false, previewTargets],
    ["daily", { event: "schedule", dryRun: false }, false, allTargets],
    ["daily", { dailyCheck: true }, false, allTargets],
    ["tag", { ref: tag }, false, allTargets],
    ["tag", { ref: tag, dryRun: false }, true, allTargets],
    ["tag", { event: "push", ref: tag, dryRun: false }, true, allTargets],
  ] as const)("plans %s runs", (mode, overrides, publish, targets) => {
    const result = releasePlan({ ...mainDispatch, ...overrides }, "0.13.0")
    expect(result.mode).toBe(mode)
    expect(result.publish).toBe(publish)
    expect(result.matrix.include.map(({ target }) => target)).toEqual(targets)
  })

  test("keeps the daily package suite on canonical main", () => {
    for (const overrides of [
      { ref: "refs/heads/feature" },
      { repo: "fork/jiyuu" },
      { dryRun: false },
      { prewarm: true },
      { ref: tag },
    ]) {
      expect(() =>
        releasePlan({ ...mainDispatch, dailyCheck: true, ...overrides }, "0.13.0")
      ).toThrow("daily_check requires")
    }
    expect(() =>
      releasePlan({ ...mainDispatch, event: "schedule", repo: "fork/jiyuu" }, "0.13.0")
    ).toThrow("Scheduled package checks require")
  })

  test("rejects a mismatched tag and a publishing request on a branch", () => {
    expect(() => releasePlan({ ...mainDispatch, ref: tag }, "0.13.1"))
      .toThrow("does not match")
    expect(() => releasePlan({ ...mainDispatch, dryRun: false }, "0.13.0"))
      .toThrow("Expected a Mori tag")
  })
})

describe("workflow dependencies", () => {
  test("each target can run parity when a different target fails", async () => {
    const packageWorkflow = ".github/workflows/mori-package-target.yml"
    const { YAML } = Bun as typeof Bun & { YAML: { parse(text: string): unknown } }
    const readWorkflow = async (path: string) =>
      YAML.parse(await Bun.file(new URL(`../../${path}`, import.meta.url)).text()) as {
        jobs: Record<string, { needs?: string[] }>
      }
    const release = await readWorkflow(".github/workflows/mori-release.yml")
    const target = await readWorkflow(packageWorkflow)

    expect(release.jobs.platform).toMatchObject({
      uses: `./${packageWorkflow}`,
      strategy: { "fail-fast": false },
    })
    expect(target.jobs.build).toBeDefined()
    expect(target.jobs.build?.needs).toBeUndefined()
    expect(target.jobs["build-reference"]).toBeDefined()
    expect(target.jobs["build-reference"]?.needs).toBeUndefined()
    expect(target.jobs["cli-parity"]?.needs).toEqual(["build", "build-reference"])
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
      const x64 = "mori-0.13.0-linux-x64.tar.gz"
      expect(checksums).toContain(
        `${await sha256File(join(dir, x64))}  ${x64}\n`,
      )
      const written = JSON.parse(
        await readFile(join(dir, "release-metadata.json"), "utf8"),
      )
      expect(written.tag).toBe("mori/v0.13.0")
      expect(written.targets).toEqual([
        "linux-x64",
        "linux-arm64",
        "darwin-arm64",
        "darwin-x64",
      ])
      const darwin = "mori-0.13.0-darwin-arm64.tar.gz"
      expect(written.assets[darwin]).toBe(
        await sha256File(join(dir, darwin)),
      )
      expect(checksums).toContain(`  ${darwin}\n`)
      const notes = await readFile(join(dir, "release-notes.md"), "utf8")
      expect(notes).toContain(COMMIT)
      expect(notes).toContain(darwin)
      expect(notes).toContain("not notarized")
      expect(notes).toContain("xattr -d com.apple.quarantine mori")
      expect(notes).toContain("Binary `mori`")
    })
  })

  test("fails when an archive is missing", async () => {
    await withDir(async (dir) => {
      await writeFile(
        join(dir, "mori-0.13.0-linux-x64.tar.gz"),
        "x",
      )
      await expect(
        prepare(dir, "0.13.0", "mori/v0.13.0", COMMIT),
      ).rejects.toThrow("linux-arm64")
    })
  })

  test("fails when only the macOS archive is missing", async () => {
    await withDir(async (dir) => {
      for (const target of ["linux-x64", "linux-arm64"]) {
        await writeFile(join(dir, assetName("0.13.0", target)), "x")
      }
      await expect(
        prepare(dir, "0.13.0", "mori/v0.13.0", COMMIT),
      ).rejects.toThrow("darwin-arm64")
    })
  })

  test("fails when only the Intel macOS archive is missing", async () => {
    await withDir(async (dir) => {
      for (const target of ["linux-x64", "linux-arm64", "darwin-arm64"]) {
        await writeFile(join(dir, assetName("0.13.0", target)), "x")
      }
      const error = await prepare(
        dir,
        "0.13.0",
        "mori/v0.13.0",
        COMMIT,
      ).catch((e: unknown) => e as Error)
      expect(error).toBeInstanceOf(Error)
      expect((error as Error).message).toBe(
        "Missing release archives: mori-0.13.0-darwin-x64.tar.gz",
      )
    })
  })

  test("prepares a branch preview with exactly the three preview archives", async () => {
    await withDir(async (dir) => {
      const targets = ["linux-x64", "linux-arm64", "darwin-arm64"]
      for (const target of targets) {
        await writeFile(join(dir, assetName("0.13.0", target)), "x")
      }
      await main(["prepare-preview", "0.13.0", dir, "--commit", COMMIT], "/")
      const meta = JSON.parse(
        await readFile(join(dir, "release-metadata.json"), "utf8"),
      )
      expect(meta.targets).toEqual(targets)
      expect(Object.keys(meta.assets)).toEqual(targets.map((target) => assetName("0.13.0", target)))
      await writeFile(join(dir, assetName("0.13.0", "darwin-x64")), "x")
      await expect(
        main(["prepare-preview", "0.13.0", dir, "--commit", COMMIT], "/"),
      ).rejects.toThrow("Unexpected package archives")
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
    expect(create).toContain(
      "/rel/mori-0.13.0-darwin-arm64.tar.gz",
    )
  })

  test("refills an existing draft", () => {
    const { run, calls } = fakeGh([
      { id: 1, tag_name: "mori/v0.13.0", draft: true },
    ])
    publish("/rel", "mori/v0.13.0", "o/r", run)
    const upload = calls.find((argv) => argv[2] === "upload")!
    expect(upload).toContain("--clobber")
    const edit = calls.find((argv) => argv[2] === "edit")!
    expect(edit).toContain("--notes-file")
    expect(edit).toContain("/rel/release-notes.md")
    expect(calls.some((argv) => argv[2] === "create")).toBe(false)
  })

  test("marks pre-release versions as prereleases", () => {
    const rc = fakeGh([])
    publish("/rel", "mori/v0.13.0-rc.1", "o/r", rc.run)
    expect(rc.calls.find((argv) => argv[2] === "create")).toContain(
      "--prerelease",
    )
    const stable = fakeGh([])
    publish("/rel", "mori/v0.13.0", "o/r", stable.run)
    expect(stable.calls.find((argv) => argv[2] === "create")).not.toContain(
      "--prerelease",
    )
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
