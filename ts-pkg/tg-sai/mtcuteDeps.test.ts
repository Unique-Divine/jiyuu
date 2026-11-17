import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"

const PKG_DIR = import.meta.dir
const pkg = JSON.parse(readFileSync(join(PKG_DIR, "package.json"), "utf8"))

/** Resolve a package's package.json as seen from `fromDir`. */
function resolvePkgJson(name: string, fromDir: string): string {
  return Bun.resolveSync(`${name}/package.json`, fromDir)
}

describe("mtcute dependency pins", () => {
  // telegramRuntime.ts calls initSync on the @mtcute/wasm it imports. If
  // @mtcute/bun loads a different copy, that copy is never initialized and
  // every connection fails with "undefined is not an object (evaluating
  // 'wasm.__malloc')". Exact pins keep one copy for any install, because a
  // published package's overrides don't apply where it's installed.
  test("every @mtcute dependency is an exact version", () => {
    const ranges = Object.entries(pkg.dependencies as Record<string, string>)
      .filter(([name]) => name.startsWith("@mtcute/"))
    expect(ranges.length).toBeGreaterThan(0)
    for (const [name, range] of ranges)
      expect({ name, range }).toEqual({
        name,
        range: expect.stringMatching(/^\d+\.\d+\.\d+$/),
      })
  })

  test("@mtcute/bun loads the same @mtcute/wasm that tg-sai initializes", () => {
    const ours = resolvePkgJson("@mtcute/wasm", PKG_DIR)
    const bunDir = dirname(resolvePkgJson("@mtcute/bun", PKG_DIR))
    expect(resolvePkgJson("@mtcute/wasm", bunDir)).toBe(ours)
  })

  // @mtcute/bun depends on the html and markdown parsers with ^ ranges. A
  // fresh install resolves those to newer releases that pull a second, newer
  // @mtcute/core, so tg-sai pins both parsers to its own core version.
  test("@mtcute/bun, convert, and both parsers share one @mtcute/core", () => {
    const core = resolvePkgJson("@mtcute/core", PKG_DIR)
    for (const dep of [
      "@mtcute/bun",
      "@mtcute/convert",
      "@mtcute/html-parser",
      "@mtcute/markdown-parser",
    ]) {
      const dir = dirname(resolvePkgJson(dep, PKG_DIR))
      expect(resolvePkgJson("@mtcute/core", dir)).toBe(core)
    }
  })

  test("@mtcute/bun's parsers are pinned to the core version", () => {
    const deps = pkg.dependencies as Record<string, string>
    const bunDeps = JSON.parse(
      readFileSync(resolvePkgJson("@mtcute/bun", PKG_DIR), "utf8"),
    ).dependencies as Record<string, string>
    const parsers = Object.keys(bunDeps).filter((name) =>
      name.endsWith("-parser"),
    )
    expect(parsers.sort()).toEqual([
      "@mtcute/html-parser",
      "@mtcute/markdown-parser",
    ])
    for (const name of parsers)
      expect({ name, range: deps[name] }).toEqual({
        name,
        range: deps["@mtcute/core"],
      })
  })
})
