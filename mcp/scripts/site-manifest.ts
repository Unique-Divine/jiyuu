// Writes mcp/<folder>/site-manifest.json for each MCP server, the data behind
// the server pages on uniquedivine.com. Run `just mcp-manifest` to write and
// `just mcp-manifest --check` to fail when a committed manifest is stale.
//
// The generator reads package manifests, the tool modules they name, and
// READMEs. Every file read goes through safePath, which resolves symlinks
// and refuses env, token, session, cookie, credential, secret, and log files
// and anything outside the server folder.

import {
  existsSync,
  lstatSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { basename, dirname, join, resolve, sep } from "node:path"

export const MANIFEST_FILE = "site-manifest.json"
export const SCHEMA_VERSION = 1
const REPO_URL = "https://github.com/Unique-Divine/jiyuu"

export type Registry = "npm" | "pypi"

export interface EnvVar {
  name: string
  required: boolean
  secret: boolean
  /** Plain-text alternatives or caveats, such as other ways to log in. */
  note?: string
}

export interface ToolParam {
  name: string
  type: string
  required: boolean
}

export interface ManifestTool {
  name: string
  title: string
  description: string
  readOnly: boolean
  destructive: boolean
  params: ToolParam[]
}

export interface SiteManifest {
  schemaVersion: number
  folder: string
  slug: string
  title: string
  description: string
  registry: Registry
  package: string
  version: string
  installCommand: string
  runtime: string
  license: string
  sourceUrl: string
  registryUrl: string
  envVars: EnvVar[]
  toolCount: number
  readOnlyCount: number
  writeCount: number
  tools: ManifestTool[]
  installMarkdown: string
  generatedFrom: { revision: string }
}

/** A tool as an MCP server lists it (tools/list result shape). */
export interface McpTool {
  name: string
  title?: string
  description?: string
  inputSchema?: { properties?: Record<string, unknown>; required?: string[] }
  annotations?: {
    title?: string
    readOnlyHint?: boolean
    destructiveHint?: boolean
  }
}

/** Site settings read from a package manifest. */
export interface ServerSource {
  folder: string
  dir: string
  registry: Registry
  package: string
  version: string
  description: string
  license: string
  runtime: string
  envVars: EnvVar[]
  /** npm: "path/to/module.ts#EXPORT" entries. pypi: one "module:function". */
  tools: string[]
  /** pypi only: synthetic env values that let the server module import. */
  toolsEnv: Record<string, string>
}

export interface Discovery {
  servers: ServerSource[]
  optedOut: string[]
}

export type PythonToolLister = (source: ServerSource) => Promise<McpTool[]>

export interface GenerateOptions {
  mcpDir: string
  /** The commit that holds the inputs used to build this server's manifest. */
  revisionFor: (source: ServerSource) => string
  listPythonTools: PythonToolLister
}

// ---------------------------------------------------------------------------
// Safe reads

const SECRET_NAME =
  /(^\.env|\.env$|token|session|cookie|credential|secret|\.log$|\.pem$|\.key$)/i

/** True when a file name looks like it holds secrets or logs. */
export function isSecretFileName(path: string): boolean {
  return SECRET_NAME.test(basename(path))
}

function inside(root: string, path: string): boolean {
  return path === root || path.startsWith(root + sep)
}

function checkPath(root: string, path: string, shown: string): void {
  if (!inside(root, path)) {
    throw new Error(`refusing to read ${shown}: outside ${root}`)
  }
  if (isSecretFileName(path)) {
    throw new Error(
      `refusing to read ${shown}: env, token, session, or log file`,
    )
  }
}

/**
 * Resolves `path` under server folder `root` and returns its real path.
 * Checks containment and the secret-name rule on both the path as written
 * and the path after following symlinks. Refuses a symlinked root.
 */
export function safePath(root: string, path: string): string {
  if (lstatSync(root).isSymbolicLink()) {
    throw new Error(`refusing symlinked server folder ${root}`)
  }
  const full = resolve(root, path)
  checkPath(resolve(root), full, path)
  const real = realpathSync(full)
  checkPath(realpathSync(root), real, `${path} (links to ${real})`)
  return real
}

/** Reads a text file through safePath. */
export function readSafeText(root: string, path: string): string {
  return readFileSync(safePath(root, path), "utf8")
}

// ---------------------------------------------------------------------------
// Discovery

function asRecord(value: unknown, what: string): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${what} must be an object`)
  }
  return value as Record<string, unknown>
}

function asString(value: unknown, what: string): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${what} must be a non-empty string`)
  }
  return value
}

function parseEnvVars(value: unknown, what: string): EnvVar[] {
  if (value === undefined) return []
  if (!Array.isArray(value)) throw new Error(`${what} must be an array`)
  return value.map((item, index) => {
    const row = asRecord(item, `${what}[${index}]`)
    const name = asString(row.name, `${what}[${index}].name`)
    if (!/^[A-Z][A-Z0-9_]*$/.test(name)) {
      throw new Error(`${what}[${index}].name is not an env var name: ${name}`)
    }
    for (const key of Object.keys(row)) {
      if (!["name", "required", "secret", "note"].includes(key)) {
        throw new Error(`${what}[${index}] has unknown key ${key}; names only`)
      }
    }
    const envVar: EnvVar = {
      name,
      required: row.required === true,
      secret: row.secret === true,
    }
    if (row.note !== undefined) {
      envVar.note = checkNote(
        asString(row.note, `${what}[${index}].note`),
        `${what}[${index}].note`,
      )
    }
    return envVar
  })
}

/** The site importer's limit for envVars[].note. */
export const NOTE_MAX_LENGTH = 300

const NOTE_REJECTS: [RegExp, string][] = [
  [/[A-Z][A-Z0-9_]*\s*=/, "an assignment such as NAME=value"],
  [/xox[a-z]-/i, "a Slack token"],
  [/[0-9a-f]{32,}/i, "a long hex run"],
  [/[A-Za-z0-9+/_-]{40,}={0,2}/, "a long base64 run"],
  [
    /\b(gh[pousr]_|github_pat_|glpat-|npm_|sk-|AKIA|AIza)|-----BEGIN/,
    "a known secret prefix",
  ],
]

/**
 * Checks an envVars note: plain text up to NOTE_MAX_LENGTH characters with
 * nothing shaped like a value. Notes name alternatives; they never hold one.
 */
export function checkNote(note: string, what: string): string {
  if (note.length > NOTE_MAX_LENGTH) {
    throw new Error(
      `${what} is ${note.length} characters; the limit is ${NOTE_MAX_LENGTH}`,
    )
  }
  for (const [pattern, why] of NOTE_REJECTS) {
    if (pattern.test(note)) {
      throw new Error(`${what} looks like it holds ${why}; notes name only`)
    }
  }
  return note
}

/** Turns ">=1.4.3" into "Bun >= 1.4.3". */
export function runtimeLabel(name: string, range: string): string {
  const match = range.trim().match(/^(>=|>|\^|~|=)?\s*(\S+)$/)
  if (!match) return `${name} ${range.trim()}`
  const op = match[1] === "=" || match[1] === undefined ? "" : `${match[1]} `
  return `${name} ${op}${match[2]}`
}

function npmSource(folder: string, dir: string): ServerSource | null {
  const pkg = asRecord(
    JSON.parse(readSafeText(dir, "package.json")),
    `${folder}/package.json`,
  )
  const jiyuu = pkg.jiyuu === undefined ? {} : asRecord(pkg.jiyuu, "jiyuu")
  if (jiyuu.site === false) return null
  const where = `${folder}/package.json jiyuu.site`
  const site = asRecord(jiyuu.site, where)
  const tools = site.tools
  if (!Array.isArray(tools) || tools.length === 0) {
    throw new Error(`${where}.tools must list "module#EXPORT" entries`)
  }
  const engines =
    pkg.engines === undefined ? {} : asRecord(pkg.engines, "engines")
  return {
    folder,
    dir,
    registry: "npm",
    package: asString(pkg.name, `${folder} name`),
    version: asString(pkg.version, `${folder} version`),
    description: asString(pkg.description, `${folder} description`),
    license: asString(pkg.license, `${folder} license`),
    runtime: runtimeLabel(
      "Bun",
      asString(engines.bun, `${folder} engines.bun`),
    ),
    envVars: parseEnvVars(site.env, `${where}.env`),
    tools: tools.map((entry, i) => asString(entry, `${where}.tools[${i}]`)),
    toolsEnv: {},
  }
}

function pypiSource(folder: string, dir: string): ServerSource | null {
  const doc = asRecord(
    Bun.TOML.parse(readSafeText(dir, "pyproject.toml")),
    `${folder}/pyproject.toml`,
  )
  const project = asRecord(doc.project, `${folder} [project]`)
  const tool = doc.tool === undefined ? {} : asRecord(doc.tool, "[tool]")
  const jiyuu =
    tool.jiyuu === undefined ? {} : asRecord(tool.jiyuu, "[tool.jiyuu]")
  if (jiyuu.site === false) return null
  const where = `${folder}/pyproject.toml [tool.jiyuu.site]`
  const site = asRecord(jiyuu.site, where)
  const license = project.license
  const toolsEnv =
    site["tools-env"] === undefined
      ? {}
      : asRecord(site["tools-env"], `${where} tools-env`)
  return {
    folder,
    dir,
    registry: "pypi",
    package: asString(project.name, `${folder} name`),
    version: asString(project.version, `${folder} version`),
    description: asString(project.description, `${folder} description`),
    license:
      typeof license === "string"
        ? license
        : asString(asRecord(license, "license").text, `${folder} license`),
    runtime: runtimeLabel(
      "Python",
      asString(project["requires-python"], `${folder} requires-python`),
    ),
    envVars: parseEnvVars(site.env, `${where} env`),
    tools: [asString(site.tools, `${where} tools`)],
    toolsEnv: Object.fromEntries(
      Object.entries(toolsEnv).map(([k, v]) => [
        k,
        asString(v, `tools-env.${k}`),
      ]),
    ),
  }
}

/** Lists mcp/<folder>/ directories and reads each package manifest. */
export function discoverServers(mcpDir: string): Discovery {
  const servers: ServerSource[] = []
  const optedOut: string[] = []
  const folders = readdirSync(mcpDir, { withFileTypes: true })
    .filter((entry) => {
      if (entry.isSymbolicLink()) {
        throw new Error(`refusing symlinked folder mcp/${entry.name}`)
      }
      return entry.isDirectory()
    })
    .map((entry) => entry.name)
    .sort()
  for (const folder of folders) {
    const dir = join(mcpDir, folder)
    let source: ServerSource | null
    if (existsSync(join(dir, "package.json"))) source = npmSource(folder, dir)
    else if (existsSync(join(dir, "pyproject.toml")))
      source = pypiSource(folder, dir)
    else continue
    if (source) servers.push(source)
    else optedOut.push(folder)
  }
  return { servers, optedOut }
}

// ---------------------------------------------------------------------------
// Tools

/** Loads "module#EXPORT" entries from an npm server and concatenates them. */
export async function loadNpmTools(source: ServerSource): Promise<McpTool[]> {
  const tools: McpTool[] = []
  for (const entry of source.tools) {
    const [path, exportName] = entry.split("#")
    if (!path || !exportName) {
      throw new Error(
        `${source.folder}: tool entry must be "module#EXPORT": ${entry}`,
      )
    }
    // Check the name and location before import runs the module.
    if (lstatSync(resolve(source.dir, path)).isSymbolicLink()) {
      throw new Error(
        `${source.folder}: refusing symlinked tool module ${path}`,
      )
    }
    const module = await import(safePath(source.dir, path))
    const value = module[exportName]
    if (value === undefined) {
      throw new Error(`${source.folder}: ${path} has no export ${exportName}`)
    }
    tools.push(...(Array.isArray(value) ? value : [value]))
  }
  return tools
}

const HELPER = join(import.meta.dir, "site_tools.py")

/** Environment for the Python helper: PATH, HOME, and synthetic values only. */
export function pythonEnv(source: ServerSource): Record<string, string> {
  const env: Record<string, string> = { JIYUU_MCP_NO_UPDATE_CHECK: "1" }
  for (const name of ["PATH", "HOME"]) {
    const value = process.env[name]
    if (value !== undefined) env[name] = value
  }
  return { ...env, ...source.toolsEnv }
}

/** Builds a lister that runs the helper with `command` plus helper args. */
export function makePythonToolLister(
  command: (source: ServerSource) => string[],
): PythonToolLister {
  return async (source) => {
    // The server may create session or log files in its working directory.
    const cwd = mkdtempSync(join(tmpdir(), "site-manifest-"))
    try {
      const spec = source.tools[0]!
      const proc = Bun.spawnSync(
        [...command(source), HELPER, source.dir, spec],
        { cwd, env: pythonEnv(source), stdout: "pipe", stderr: "pipe" },
      )
      if (proc.exitCode !== 0) {
        throw new Error(
          `${source.folder}: listing tools with ${spec} failed\n` +
            proc.stderr.toString(),
        )
      }
      return JSON.parse(proc.stdout.toString()) as McpTool[]
    } finally {
      rmSync(cwd, { recursive: true, force: true })
    }
  }
}

/** Runs the helper in the server's locked uv environment. */
export const uvToolLister = makePythonToolLister((source) => [
  "uv",
  "run",
  "--frozen",
  "--quiet",
  "--project",
  source.dir,
  "python",
  "-I",
])

/** First paragraph of a description, before any "Args:" block, on one line. */
export function summarize(text: string | undefined): string {
  const lines = (text ?? "").trim().split("\n")
  const kept: string[] = []
  for (const line of lines) {
    if (line.trim() === "" || /^\s*(Args|Returns|Note):/.test(line)) break
    kept.push(line.trim())
  }
  return kept.join(" ")
}

function collectTypes(schema: unknown): string[] {
  if (schema === null || typeof schema !== "object") return ["any"]
  const s = schema as Record<string, unknown>
  if (typeof s.type === "string") return [s.type]
  if (Array.isArray(s.type)) return s.type.map(String)
  const options = Array.isArray(s.anyOf) ? s.anyOf : s.oneOf
  if (Array.isArray(options)) return options.flatMap(collectTypes)
  return ["any"]
}

/** JSON Schema type as "integer | string"; a nullable type drops "null". */
export function schemaType(schema: unknown): string {
  const kept = [...new Set(collectTypes(schema).filter((t) => t !== "null"))]
  return kept.length === 0 ? "any" : kept.join(" | ")
}

/** Maps an MCP tool to the manifest shape, using MCP's annotation defaults. */
export function toManifestTool(tool: McpTool): ManifestTool {
  const annotations = tool.annotations ?? {}
  const readOnly = annotations.readOnlyHint === true
  const properties = tool.inputSchema?.properties ?? {}
  const required = new Set(tool.inputSchema?.required ?? [])
  return {
    name: tool.name,
    title: tool.title ?? annotations.title ?? tool.name,
    description: summarize(tool.description),
    readOnly,
    // MCP treats a missing destructiveHint on a write tool as true.
    destructive: !readOnly && annotations.destructiveHint !== false,
    params: Object.entries(properties).map(([name, schema]) => ({
      name,
      type: schemaType(schema),
      required: required.has(name),
    })),
  }
}

// ---------------------------------------------------------------------------
// README

/** Body of the README's "## Install" section, without the heading. */
export function installSection(readme: string): string {
  const lines = readme.split("\n")
  let inSection = false
  let found = false
  let inFence = false
  const body: string[] = []
  for (const line of lines) {
    if (/^\s*(```|~~~)/.test(line)) inFence = !inFence
    if (!inFence && /^## /.test(line)) {
      if (inSection) break
      inSection = line.trim() === "## Install"
      found ||= inSection
      continue
    }
    if (inSection) body.push(line)
  }
  if (!found) throw new Error('README has no "## Install" section')
  return body.join("\n").trim()
}

/** The README's first "# " heading. */
export function readmeTitle(readme: string): string {
  const match = readme.match(/^# (.+)$/m)
  if (!match) throw new Error("README has no # title")
  return match[1]!.trim()
}

// ---------------------------------------------------------------------------
// Branding guard

const BANNED: [RegExp, string][] = [
  [/revasser/i, "upstream vendor name"],
  [/jtalk22|barryyip|chigwell|l1v0n1/i, "upstream author or repository"],
  [/\$\s?\d/, "price"],
  [/\/mo(nth)?\b|per month/i, "price"],
  [/pricing|free tier|upgrade to pro|\bPro or Team\b/i, "pricing"],
  [/\bhosted\b/i, "hosted-service text"],
]

/** Throws on an upstream vendor, price, or hosted service in the text. */
export function assertNoUpstreamBranding(manifest: SiteManifest): void {
  const text = JSON.stringify(manifest)
  for (const [pattern, why] of BANNED) {
    const match = text.match(pattern)
    if (match) {
      throw new Error(
        `${manifest.folder}: manifest contains ${why} ` +
          `(${JSON.stringify(match[0])}); ` +
          "fix the package's own description or tool metadata",
      )
    }
  }
}

// ---------------------------------------------------------------------------
// Build

export function buildManifest(
  source: ServerSource,
  mcpTools: McpTool[],
  readme: string,
  revision: string,
): SiteManifest {
  const tools = mcpTools.map(toManifestTool)
  const names = tools.map((t) => t.name)
  const duplicate = names.find((name, i) => names.indexOf(name) !== i)
  if (duplicate)
    throw new Error(`${source.folder}: duplicate tool ${duplicate}`)
  if (!names.includes("mcp_info")) {
    throw new Error(`${source.folder}: tool list has no mcp_info`)
  }
  const readOnlyCount = tools.filter((t) => t.readOnly).length
  const npm = source.registry === "npm"
  const manifest: SiteManifest = {
    schemaVersion: SCHEMA_VERSION,
    folder: source.folder,
    slug: `${source.folder}-mcp`,
    title: readmeTitle(readme),
    description: source.description,
    registry: source.registry,
    package: source.package,
    version: source.version,
    // Tag @latest makes bunx and uvx check the registry at each launch.
    // A bare uvx name reuses its first cached install and never updates.
    installCommand: `${npm ? "bunx" : "uvx"} ${source.package}@latest`,
    runtime: source.runtime,
    license: source.license,
    sourceUrl: `${REPO_URL}/tree/main/mcp/${source.folder}`,
    registryUrl: npm
      ? `https://www.npmjs.com/package/${source.package}`
      : `https://pypi.org/project/${source.package}/`,
    envVars: source.envVars,
    toolCount: tools.length,
    readOnlyCount,
    writeCount: tools.length - readOnlyCount,
    tools,
    installMarkdown: installSection(readme),
    generatedFrom: { revision },
  }
  assertNoUpstreamBranding(manifest)
  return manifest
}

export function serialize(manifest: SiteManifest): string {
  return JSON.stringify(manifest, null, 2) + "\n"
}

/** Problem with a committed generatedFrom object, or null when valid. */
export function generatedFromProblem(value: unknown): string | null {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return "generatedFrom must be an object"
  }
  const keys = Object.keys(value)
  if (keys.length !== 1 || keys[0] !== "revision") {
    return "generatedFrom must hold only revision"
  }
  const revision = (value as { revision: unknown }).revision
  if (typeof revision !== "string" || !/^[0-9a-f]{7,40}$/.test(revision)) {
    return "generatedFrom.revision must be a git commit hash"
  }
  return null
}

export type Status = "fresh" | "stale" | "revision" | "missing" | "unwanted"

/**
 * Compares a committed manifest with a freshly built one. "stale" means the
 * content differs or generatedFrom is malformed; "revision" means only the
 * revision value differs. --check accepts "revision", because a shallow CI
 * checkout can't always find the input commit.
 */
export function compare(current: string, next: string): Status {
  let parsed: Record<string, unknown>
  try {
    parsed = JSON.parse(current)
  } catch {
    return "stale"
  }
  if (generatedFromProblem(parsed.generatedFrom) !== null) return "stale"
  const wanted = JSON.parse(next) as Record<string, unknown>
  const revision = (parsed.generatedFrom as { revision: string }).revision
  const same = (a: Record<string, unknown>, b: Record<string, unknown>) =>
    JSON.stringify({ ...a, generatedFrom: null }) ===
    JSON.stringify({ ...b, generatedFrom: null })
  if (!same(parsed, wanted)) return "stale"
  const want = (wanted.generatedFrom as { revision: string }).revision
  return revision === want ? "fresh" : "revision"
}

export interface Outcome {
  folder: string
  path: string
  status: Status
  text?: string
}

/** Computes each server's manifest and compares it with the file on disk. */
export async function plan(options: GenerateOptions): Promise<Outcome[]> {
  const { servers, optedOut } = discoverServers(options.mcpDir)
  const outcomes: Outcome[] = []
  for (const source of servers) {
    const mcpTools =
      source.registry === "npm"
        ? await loadNpmTools(source)
        : await options.listPythonTools(source)
    const readme = readSafeText(source.dir, "README.md")
    const text = serialize(
      buildManifest(source, mcpTools, readme, options.revisionFor(source)),
    )
    const path = join(source.dir, MANIFEST_FILE)
    if (!existsSync(path)) {
      outcomes.push({ folder: source.folder, path, status: "missing", text })
      continue
    }
    const current = readSafeText(source.dir, MANIFEST_FILE)
    outcomes.push({
      folder: source.folder,
      path,
      status: compare(current, text),
      text,
    })
  }
  for (const folder of optedOut) {
    const path = join(options.mcpDir, folder, MANIFEST_FILE)
    if (existsSync(path)) outcomes.push({ folder, path, status: "unwanted" })
  }
  return outcomes
}

/** Writes changed and missing manifests and removes opted-out ones. */
export function apply(outcomes: Outcome[]): void {
  for (const outcome of outcomes) {
    if (outcome.status === "unwanted") unlinkSync(outcome.path)
    else if (outcome.status !== "fresh")
      writeFileSync(outcome.path, outcome.text!)
  }
}

/** Statuses that fail --check. */
export function failsCheck(status: Status): boolean {
  return status !== "fresh" && status !== "revision"
}

export function parseArgs(args: readonly string[]): { check: boolean } {
  let check = false
  for (const arg of args) {
    if (arg === "--check") check = true
    else throw new Error("Usage: just mcp-manifest [--check]")
  }
  return { check }
}

/**
 * Git pathspecs for a server's manifest inputs: its folder except the
 * manifest itself, plus the generator and helper in mcp/scripts.
 */
export function inputPathspecs(folder: string): string[] {
  return [
    `mcp/${folder}`,
    `:(exclude)mcp/${folder}/${MANIFEST_FILE}`,
    "mcp/scripts",
  ]
}

function git(repo: string, args: string[]): string {
  const proc = Bun.spawnSync(["git", ...args], { cwd: repo })
  if (proc.exitCode !== 0) {
    throw new Error(`git ${args.join(" ")} failed\n${proc.stderr}`)
  }
  // trimEnd keeps the leading space of porcelain status lines.
  return proc.stdout.toString().trimEnd()
}

/** The last commit that changed a server's manifest inputs. */
export function inputRevision(repo: string, folder: string): string {
  const revision = git(repo, [
    "log",
    "-1",
    "--format=%H",
    "--",
    ...inputPathspecs(folder),
  ])
  if (!revision) throw new Error(`no commit touches mcp/${folder}`)
  return revision
}

/** Uncommitted changes to a server's manifest inputs, as porcelain lines. */
export function dirtyInputs(repo: string, folder: string): string[] {
  const out = git(repo, [
    "status",
    "--porcelain",
    "--untracked-files=all",
    "--",
    ...inputPathspecs(folder),
  ])
  return out === "" ? [] : out.split("\n")
}

export async function main(args: readonly string[]): Promise<number> {
  const { check } = parseArgs(args)
  const mcpDir = resolve(dirname(import.meta.dir))
  const repo = dirname(mcpDir)
  const outcomes = await plan({
    mcpDir,
    revisionFor: (source) => {
      // A revision names committed inputs, so writing needs a clean tree.
      const dirty = check ? [] : dirtyInputs(repo, source.folder)
      if (dirty.length > 0) {
        throw new Error(
          `commit the changes to mcp/${source.folder} inputs first:\n` +
            dirty.join("\n"),
        )
      }
      return inputRevision(repo, source.folder)
    },
    listPythonTools: uvToolLister,
  })
  const changed = outcomes.filter((o) => o.status !== "fresh")
  for (const o of outcomes) {
    const rel = o.path.slice(dirname(mcpDir).length + 1)
    console.log(`${o.status.padEnd(8)} ${rel}`)
  }
  if (check) {
    if (outcomes.some((o) => failsCheck(o.status))) {
      console.error("Site manifests are out of date. Run: just mcp-manifest")
      return 1
    }
    return 0
  }
  apply(changed)
  return 0
}

if (import.meta.main) {
  try {
    process.exit(await main(Bun.argv.slice(2)))
  } catch (error) {
    console.error(error instanceof Error ? error.message : error)
    process.exit(1)
  }
}
