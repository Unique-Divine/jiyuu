/**
 * `tg-sai profile migrate-password`: moves plain-text 2FA passwords from
 * `profiles.json` into pass, then removes password fields from
 * `backups/*.json`. Dry-run by default.
 */
import { readdir, readFile, stat } from "node:fs/promises"
import { join, relative } from "node:path"
import {
  configDir,
  findProfileIndex,
  plainTextPasswordProfiles,
  readConfig,
  unknownProfileError,
  writeConfig,
  writePrivateFile,
} from "./profiles"
import type { ProfileStoreEnv, TgSaiProfile } from "./profiles"
import {
  assertPassReady,
  defaultPasswordPassEntry,
  insertPassPassword,
  passEntryExists,
  readPassPassword,
  validatePassEntry,
} from "./passwordStore"

export interface MigratePasswordOptions {
  /** Profile selector; omit to migrate every profile with a plain password. */
  profile?: string
  /** Pass entry; only with `profile`. Defaults per profile. */
  passEntry?: string
  /** Apply changes; default is a dry run. */
  run?: boolean
}

/** Keys that hold a 2FA password in profiles or credential blocks. */
export const PASSWORD_KEYS = ["password", "TELEGRAM_PASSWORD"] as const

/** JSON value with its source text kept for strings, numbers, and literals. */
type JsonNode =
  | { kind: "object"; members: Array<{ keyRaw: string; key: string; value: JsonNode }> }
  | { kind: "array"; items: JsonNode[] }
  | { kind: "raw"; raw: string }

/**
 * Parses JSON text that `JSON.parse` already accepted, keeping each scalar's
 * source text. Numbers stay exact, so a 64-bit ID is never rounded.
 */
export function parseJsonKeepingScalars(text: string): JsonNode {
  let i = 0
  const skip = () => {
    while (i < text.length && /\s/.test(text[i]!)) i++
  }
  const readString = (): string => {
    const start = i
    i++
    while (text[i] !== '"') i += text[i] === "\\" ? 2 : 1
    i++
    return text.slice(start, i)
  }
  const readValue = (): JsonNode => {
    skip()
    const char = text[i]
    if (char === "{") {
      i++
      const members: Array<{ keyRaw: string; key: string; value: JsonNode }> = []
      skip()
      if (text[i] === "}") {
        i++
        return { kind: "object", members }
      }
      for (;;) {
        skip()
        const keyRaw = readString()
        skip()
        i++ // :
        const value = readValue()
        members.push({ keyRaw, key: JSON.parse(keyRaw) as string, value })
        skip()
        if (text[i++] === "}") return { kind: "object", members }
      }
    }
    if (char === "[") {
      i++
      const items: JsonNode[] = []
      skip()
      if (text[i] === "]") {
        i++
        return { kind: "array", items }
      }
      for (;;) {
        items.push(readValue())
        skip()
        if (text[i++] === "]") return { kind: "array", items }
      }
    }
    if (char === '"') return { kind: "raw", raw: readString() }
    const start = i
    while (i < text.length && !/[\s,\]}]/.test(text[i]!)) i++
    return { kind: "raw", raw: text.slice(start, i) }
  }
  return readValue()
}

/** Renders like `JSON.stringify(value, null, 2)`, scalars as written. */
export function renderJson(node: JsonNode, depth = 0): string {
  if (node.kind === "raw") return node.raw
  const pad = "  ".repeat(depth + 1)
  const end = "  ".repeat(depth)
  if (node.kind === "array") {
    if (node.items.length === 0) return "[]"
    return `[\n${node.items.map((item) => pad + renderJson(item, depth + 1)).join(",\n")}\n${end}]`
  }
  if (node.members.length === 0) return "{}"
  return `{\n${node.members
    .map((member) => `${pad}${member.keyRaw}: ${renderJson(member.value, depth + 1)}`)
    .join(",\n")}\n${end}}`
}

function scrubNode(node: JsonNode): { node: JsonNode; removed: number } {
  if (node.kind === "raw") return { node, removed: 0 }
  let removed = 0
  if (node.kind === "array") {
    const items = node.items.map((item) => {
      const scrubbed = scrubNode(item)
      removed += scrubbed.removed
      return scrubbed.node
    })
    return { node: { kind: "array", items }, removed }
  }
  const members = []
  for (const member of node.members) {
    if ((PASSWORD_KEYS as readonly string[]).includes(member.key)) {
      const value = member.value
      if (value.kind !== "raw" || (value.raw !== '""' && value.raw !== "null")) {
        removed++
      }
      continue
    }
    const scrubbed = scrubNode(member.value)
    removed += scrubbed.removed
    members.push({ ...member, value: scrubbed.node })
  }
  return { node: { kind: "object", members }, removed }
}

/**
 * Removes `password` and `TELEGRAM_PASSWORD` keys at any depth from JSON
 * text and counts the non-empty ones. Other values keep their exact source
 * text. Throws `SyntaxError` for text that is not JSON.
 */
export function scrubPasswordText(text: string): { text: string; removed: number } {
  JSON.parse(text)
  const scrubbed = scrubNode(parseJsonKeepingScalars(text))
  return { text: `${renderJson(scrubbed.node)}\n`, removed: scrubbed.removed }
}

interface BackupPlan {
  path: string
  /** Fields to remove, or null when the file is not JSON. */
  removed: number | null
  scrubbed: string
}

async function backupPlans(dir: string): Promise<BackupPlan[]> {
  const backupsDir = join(dir, "backups")
  let names: string[]
  try {
    names = await readdir(backupsDir)
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return []
    throw error
  }
  const plans: BackupPlan[] = []
  for (const name of names.filter((file) => file.endsWith(".json")).sort()) {
    const path = join(backupsDir, name)
    try {
      const scrubbed = scrubPasswordText(await readFile(path, "utf8"))
      plans.push({ path, removed: scrubbed.removed, scrubbed: scrubbed.text })
    } catch (error) {
      if (error instanceof SyntaxError) {
        plans.push({ path, removed: null, scrubbed: "" })
        continue
      }
      throw error
    }
  }
  return plans
}

/** Files to scan under a dir, plus symlinks the scan cannot follow. */
interface ScanFiles {
  files: string[]
  /** Symlinks to directories or missing targets; checked by hand. */
  unchecked: string[]
}

/**
 * Regular files under `dir`, including symlinks to files (read through the
 * link). Symlinked directories are not followed, to avoid loops, and are
 * listed in `unchecked` with broken links.
 */
async function filesUnder(dir: string, out: ScanFiles = { files: [], unchecked: [] }): Promise<ScanFiles> {
  let entries
  try {
    entries = await readdir(dir, { withFileTypes: true })
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return out
    throw error
  }
  for (const entry of entries) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) await filesUnder(path, out)
    else if (entry.isFile()) out.files.push(path)
    else if (entry.isSymbolicLink()) {
      const target = await stat(path).catch(() => null)
      if (target?.isFile()) out.files.push(path)
      else out.unchecked.push(path)
    }
  }
  return out
}

/** Result of scanning a dir for secrets: paths only, never values. */
export interface SecretScan {
  hits: string[]
  unchecked: string[]
}

/**
 * Scans files under `dir` for any of `secrets`. Returns matching paths and
 * symlinks it could not follow; never the matching value.
 */
export async function scanForSecrets(
  dir: string,
  secrets: readonly string[],
): Promise<SecretScan> {
  const needles = secrets.filter(Boolean).map((secret) => Buffer.from(secret))
  if (needles.length === 0) return { hits: [], unchecked: [] }
  const { files, unchecked } = await filesUnder(dir)
  const hits: string[] = []
  for (const path of files) {
    if ((await stat(path)).size === 0) continue
    const bytes = await readFile(path)
    if (needles.some((needle) => bytes.includes(needle))) hits.push(path)
  }
  return { hits, unchecked }
}

/** Paths under `dir` whose bytes contain any of `secrets`. */
export async function filesContaining(
  dir: string,
  secrets: readonly string[],
): Promise<string[]> {
  return (await scanForSecrets(dir, secrets)).hits
}

interface ProfileTarget {
  profile: TgSaiProfile
  entry: string
}

function targetsFor(
  profiles: TgSaiProfile[],
  allProfiles: TgSaiProfile[],
  options: MigratePasswordOptions,
): ProfileTarget[] {
  if (options.passEntry !== undefined && options.profile === undefined) {
    throw new Error("--pass-entry needs --profile")
  }
  if (options.profile !== undefined) {
    const index = findProfileIndex({ profiles: allProfiles }, options.profile)
    if (index === -1) throw unknownProfileError(options.profile)
    const profile = allProfiles[index]!
    if (!profiles.includes(profile)) return []
    return [
      {
        profile,
        entry: validatePassEntry(
          options.passEntry ?? defaultPasswordPassEntry(profile.name),
        ),
      },
    ]
  }
  return profiles.map((profile) => ({
    profile,
    entry: defaultPasswordPassEntry(profile.name),
  }))
}

/**
 * Plans or applies the migration and returns report lines. Lines name
 * profiles, pass entries, and file paths, never password values.
 *
 * Order on `--run`: write each pass entry and read it back, then rewrite
 * `profiles.json`, then scrub backups, then scan the state directory.
 */
export async function migrateProfilePasswords(
  options: MigratePasswordOptions,
  env: ProfileStoreEnv = process.env,
): Promise<{ lines: string[]; exitCode: number }> {
  const dir = configDir(env)
  const lines: string[] = [`mode: ${options.run ? "run" : "dry_run"}`]
  const config = await readConfig(env)
  const targets = targetsFor(
    plainTextPasswordProfiles(config),
    config.profiles,
    options,
  )
  const passwords = targets.map((target) => target.profile.password)
  // With --profile, the scan reads only that profile's pass entry.
  const scope =
    options.profile === undefined
      ? config.profiles
      : config.profiles.filter(
          (_, index) => index === findProfileIndex(config, options.profile!),
        )
  const passProfiles = scope.filter(
    (profile) => profile.passwordPassEntry && !targets.some((t) => t.profile === profile),
  )
  if (options.run && (targets.length > 0 || passProfiles.length > 0)) {
    await assertPassReady(env)
  }

  if (targets.length === 0) {
    lines.push("profiles: none with a plain-text password")
  }
  for (const target of targets) {
    const exists = await passEntryExists(target.entry, env)
    lines.push(`profile: ${target.profile.name}`)
    lines.push(`  pass_entry: ${target.entry}`)
    if (!options.run) {
      lines.push(
        exists
          ? "  pass: entry exists; --run checks that it holds the same password"
          : "  pass: insert entry",
      )
      lines.push("  profiles_json: replace password with passwordPassEntry")
      continue
    }
    if (exists) {
      const current = await readPassPassword(target.entry, env)
      if (current !== target.profile.password) {
        throw new Error(
          `pass entry ${target.entry} already holds a different password. Choose another --pass-entry; nothing was changed for profile ${target.profile.name}.`,
        )
      }
      lines.push("  pass: entry already holds this password")
    } else {
      await insertPassPassword(target.entry, target.profile.password, env)
      if ((await readPassPassword(target.entry, env)) !== target.profile.password) {
        throw new Error(`pass entry ${target.entry} did not read back the saved password`)
      }
      lines.push("  pass: inserted and read back")
    }
    target.profile.passwordPassEntry = target.entry
    target.profile.password = ""
    lines.push("  profiles_json: password replaced with passwordPassEntry")
  }
  if (options.run && targets.length > 0) {
    await writeConfig(config, env)
  }

  for (const plan of await backupPlans(dir)) {
    const label = relative(dir, plan.path)
    if (plan.removed === null) {
      lines.push(`backup: ${label} not JSON; check it by hand`)
      continue
    }
    if (plan.removed === 0) {
      lines.push(`backup: ${label} no password fields`)
      continue
    }
    if (options.run) {
      await writePrivateFile(plan.path, plan.scrubbed)
      lines.push(`backup: ${label} removed ${plan.removed} password field(s)`)
    } else {
      lines.push(`backup: ${label} remove ${plan.removed} password field(s)`)
    }
  }

  let exitCode = 0
  if (options.run) {
    // Also scan for passwords moved by earlier runs, so a rerun keeps
    // reporting a leftover copy until it is removed.
    for (const profile of passProfiles) {
      try {
        passwords.push(await readPassPassword(profile.passwordPassEntry!, env))
      } catch {
        lines.push(
          `scan: could not read pass entry ${profile.passwordPassEntry}; scan skipped profile ${profile.name}`,
        )
        exitCode = 1
      }
    }
    const scan = await scanForSecrets(dir, passwords)
    for (const path of scan.hits) {
      lines.push(`residual: ${relative(dir, path)} still contains a migrated password`)
    }
    for (const path of scan.unchecked) {
      lines.push(`residual: ${relative(dir, path)} is a symlink the scan does not follow; check its target`)
    }
    lines.push(`residual_files: ${scan.hits.length + scan.unchecked.length}`)
    if (scan.hits.length + scan.unchecked.length > 0) exitCode = 1
  } else {
    lines.push("next: rerun with --run to apply")
  }
  return { lines, exitCode }
}
