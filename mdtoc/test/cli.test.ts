import { afterEach, expect, test } from "bun:test"
import {
  cpSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { version } from "../package.json"

const root = join(import.meta.dir, "..")
const cli = join(root, "src/cli.ts")
const directories: string[] = []
const sample = "# Title\n\n## One\n\n### Two\n\n#### Hidden\n"
const flags = ["--bullets=-", "--maxdepth=3", "--no-firsth1"]
const expected = "- [One](#one)\n  - [Two](#two)"

function tempDirectory(): string {
  const path = mkdtempSync(join(tmpdir(), "ud-mdtoc-test-"))
  directories.push(path)
  return path
}

function run(args: string[], input?: string) {
  return Bun.spawnSync([process.execPath, "--no-install", cli, ...args], {
    stdin: input === undefined ? undefined : Buffer.from(input),
    stdout: "pipe",
    stderr: "pipe",
  })
}

afterEach(() => {
  for (const path of directories.splice(0)) rmSync(path, { recursive: true })
})

test("CLI exposes successful help, version, and ud plugin metadata", () => {
  for (const flag of ["--help", "-h"]) {
    const result = run([flag])
    expect(result.exitCode).toBe(0)
    expect(result.stdout.toString()).toContain("Usage: ud-mdtoc")
  }
  const result = run(["--version"])
  expect(result.exitCode).toBe(0)
  expect(result.stdout.toString().trim()).toBe(`ud-mdtoc ${version}`)
  const metadata = run(["--plugin-info"])
  expect(metadata.exitCode).toBe(0)
  expect(JSON.parse(metadata.stdout.toString())).toEqual({
    apiVersion: 1,
    name: "mdtoc",
    description: "Generate Markdown tables of contents",
  })
})

test("CLI generates the Neovim TOC from stdin with string bullets", () => {
  const result = run(["-", ...flags], sample)
  expect(result.exitCode).toBe(0)
  expect(result.stdout.toString()).toBe(expected)
  expect(result.stderr.toString()).toBe("")
})

test("CLI supports filenames with spaces and JSON output", () => {
  const file = join(tempDirectory(), "sample notes.md")
  writeFileSync(file, sample)
  const result = run([file, ...flags])
  expect(result.exitCode).toBe(0)
  expect(result.stdout.toString()).toBe(expected)
  const json = run([file, "--json", ...flags])
  expect(json.exitCode).toBe(0)
  expect(
    JSON.parse(json.stdout.toString()).map(
      (entry: { content: string }) => entry.content,
    ),
  ).toEqual(["One", "Two", "Hidden"])
})

test("CLI replaces undefined bullets and repeated in-place updates are stable", () => {
  const file = join(tempDirectory(), "sample notes.md")
  writeFileSync(
    file,
    "# Title\n\n<!-- toc -->\nundefined [Old](#old)\n<!-- tocstop -->\n\n## One\n\n### Two\n\n#### Hidden\n",
  )
  const first = run([file, "-i", ...flags])
  expect(first.exitCode).toBe(0)
  const updated = readFileSync(file, "utf8")
  expect(updated).toContain(`<!-- toc -->\n${expected}\n<!-- tocstop -->`)
  expect(updated).not.toContain("undefined")
  const second = run([file, "-i", ...flags])
  expect(second.exitCode).toBe(0)
  expect(readFileSync(file, "utf8")).toBe(updated)
})

test("CLI rejects conflicting options, stdin writes, and missing inputs", () => {
  expect(run([]).exitCode).not.toBe(0)
  expect(run(["-", "-i"], sample).exitCode).not.toBe(0)
  expect(run(["-", "-i", "--json"], sample).exitCode).not.toBe(0)
  expect(run([join(tempDirectory(), "missing.md")]).exitCode).not.toBe(0)
})

test("executable refuses missing dependencies instead of loading cached packages", () => {
  const isolated = tempDirectory()
  cpSync(join(root, "src"), join(isolated, "src"), { recursive: true })
  cpSync(join(root, "package.json"), join(isolated, "package.json"))
  const result = Bun.spawnSync([join(isolated, "src/cli.ts"), "-", ...flags], {
    cwd: isolated,
    stdin: Buffer.from(sample),
    stdout: "pipe",
    stderr: "pipe",
  })
  expect(result.exitCode).not.toBe(0)
  expect(result.stdout.toString()).not.toContain("undefined [")
  expect(result.stderr.toString()).toMatch(/Cannot find (module|package)/)
})
