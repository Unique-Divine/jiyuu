import assert from "node:assert/strict"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { name, version } from "../package.json"

const root = join(import.meta.dir, "..")
const temporary = mkdtempSync(join(tmpdir(), "ud-mdtoc-pack-"))
const config = join(temporary, "bunfig.toml")
const globalDirectory = join(temporary, "global")
const binDirectory = join(temporary, "bin")
const environment = {
  ...process.env,
  BUN_INSTALL_GLOBAL_DIR: globalDirectory,
  BUN_INSTALL_BIN: binDirectory,
}
const executable = join(binDirectory, "ud-mdtoc")
const tarball = join(temporary, `uniquedivine-ud-mdtoc-${version}.tgz`)
const sample = "# Title\n\n## One\n\n### Two\n\n#### Hidden\n"
const expected = "- [One](#one)\n  - [Two](#two)"
const flags = ["--bullets=-", "--maxdepth=3", "--no-firsth1"]

function run(command: string[], cwd = temporary, input?: string): string {
  const result = Bun.spawnSync(command, {
    cwd,
    env: environment,
    stdin: input === undefined ? undefined : Buffer.from(input),
    stdout: "pipe",
    stderr: "pipe",
  })
  assert.equal(
    result.exitCode,
    0,
    `${command.join(" ")}\n${result.stdout}\n${result.stderr}`,
  )
  return result.stdout.toString()
}

try {
  writeFileSync(
    config,
    `[install]\nglobalDir = ${JSON.stringify(globalDirectory)}\nglobalBinDir = ${JSON.stringify(binDirectory)}\n`,
  )
  run([process.execPath, "pm", "pack", "--destination", temporary], root)
  const contents = run(["tar", "-tzf", tarball])
  for (const file of [
    "src/cli.ts",
    "src/mdtoc/generate.ts",
    "src/mdtoc/insert.ts",
    "LICENSE",
    "README.md",
  ]) {
    assert.ok(contents.includes(`package/${file}`), `tarball missing ${file}`)
  }
  assert.ok(!contents.includes("package/test/"))
  run([process.execPath, "add", "--global", `--config=${config}`, tarball])
  assert.ok(run([executable, "--help"]).includes("Usage: ud-mdtoc"))
  assert.equal(run([executable, "--version"]).trim(), `ud-mdtoc ${version}`)
  assert.deepEqual(JSON.parse(run([executable, "--plugin-info"])), {
    apiVersion: 1,
    name: "mdtoc",
    description: "Generate Markdown tables of contents",
  })
  assert.equal(run([executable, "-", ...flags], temporary, sample), expected)
  const file = join(temporary, "sample notes.md")
  writeFileSync(
    file,
    "# Title\n\n<!-- toc -->\nundefined [Old](#old)\n<!-- tocstop -->\n\n## One\n\n### Two\n\n#### Hidden\n",
  )
  run([executable, file, "-i", ...flags])
  const updated = readFileSync(file, "utf8")
  assert.ok(updated.includes(`<!-- toc -->\n${expected}\n<!-- tocstop -->`))
  run([executable, file, "-i", ...flags])
  assert.equal(readFileSync(file, "utf8"), updated)
  // A broken installation must not fall back to another cached dependency.
  rmSync(join(globalDirectory, "node_modules/list-item"), { recursive: true })
  const broken = Bun.spawnSync([executable, "-", ...flags], {
    cwd: temporary,
    env: environment,
    stdin: Buffer.from(sample),
    stdout: "pipe",
    stderr: "pipe",
  })
  assert.notEqual(broken.exitCode, 0)
  assert.ok(!broken.stdout.toString().includes("undefined ["))
  console.log(
    `${name}@${version}: packed global installation passed smoke tests`,
  )
} finally {
  rmSync(temporary, { recursive: true, force: true })
}
