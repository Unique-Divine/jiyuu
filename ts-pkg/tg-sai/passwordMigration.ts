/**
 * `tg-sai profile migrate-password`: moves plain-text 2FA passwords from
 * `profiles.json` into pass, then removes password fields from
 * `backups/*.json`. Dry-run by default.
 */
import { chmod, readdir, readFile, rename, stat, writeFile } from "node:fs/promises"
import { join, relative } from "node:path"
import {
  configDir,
  findProfileIndex,
  plainTextPasswordProfiles,
  readConfig,
  unknownProfileError,
  writeConfig,
} from "./profiles"
import type { ProfileStoreEnv, TgSaiProfile } from "./profiles"
import {
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

/** Removes non-empty `password`/`TELEGRAM_PASSWORD` keys at any depth. */
export function scrubPasswordFields(value: unknown): {
  value: unknown
  removed: number
} {
  if (Array.isArray(value)) {
    let removed = 0
    const items = value.map((item) => {
      const scrubbed = scrubPasswordFields(item)
      removed += scrubbed.removed
      return scrubbed.value
    })
    return { value: items, removed }
  }
  if (typeof value !== "object" || value === null) {
    return { value, removed: 0 }
  }
  let removed = 0
  const out: Record<string, unknown> = {}
  for (const [key, child] of Object.entries(value)) {
    if ((PASSWORD_KEYS as readonly string[]).includes(key)) {
      if (child !== "" && child !== null && child !== undefined) removed++
      continue
    }
    const scrubbed = scrubPasswordFields(child)
    removed += scrubbed.removed
    out[key] = scrubbed.value
  }
  return { value: out, removed }
}

interface BackupPlan {
  path: string
  /** Fields to remove, or null when the file is not JSON. */
  removed: number | null
  scrubbed: unknown
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
    let parsed: unknown
    try {
      parsed = JSON.parse(await readFile(path, "utf8"))
    } catch (error) {
      if (error instanceof SyntaxError) {
        plans.push({ path, removed: null, scrubbed: null })
        continue
      }
      throw error
    }
    const scrubbed = scrubPasswordFields(parsed)
    plans.push({ path, removed: scrubbed.removed, scrubbed: scrubbed.value })
  }
  return plans
}

/** Replaces `path` through a temp file, keeping mode 0600. */
async function writePrivateFile(path: string, body: string): Promise<void> {
  const temp = `${path}.tmp-${process.pid}`
  await writeFile(temp, body, { mode: 0o600 })
  await chmod(temp, 0o600)
  await rename(temp, path)
}

async function filesUnder(dir: string): Promise<string[]> {
  const out: string[] = []
  let entries
  try {
    entries = await readdir(dir, { withFileTypes: true })
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return out
    throw error
  }
  for (const entry of entries) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...(await filesUnder(path)))
    else if (entry.isFile()) out.push(path)
  }
  return out
}

/**
 * Files under `dir` whose bytes contain any of `secrets`. Returns paths
 * only; never the matching value.
 */
export async function filesContaining(
  dir: string,
  secrets: readonly string[],
): Promise<string[]> {
  const needles = secrets.filter(Boolean).map((secret) => Buffer.from(secret))
  if (needles.length === 0) return []
  const hits: string[] = []
  for (const path of await filesUnder(dir)) {
    if ((await stat(path)).size === 0) continue
    const bytes = await readFile(path)
    if (needles.some((needle) => bytes.includes(needle))) hits.push(path)
  }
  return hits
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
      await writePrivateFile(plan.path, `${JSON.stringify(plan.scrubbed, null, 2)}\n`)
      lines.push(`backup: ${label} removed ${plan.removed} password field(s)`)
    } else {
      lines.push(`backup: ${label} remove ${plan.removed} password field(s)`)
    }
  }

  let exitCode = 0
  if (options.run) {
    const residual = await filesContaining(dir, passwords)
    for (const path of residual) {
      lines.push(`residual: ${relative(dir, path)} still contains a migrated password`)
    }
    lines.push(`residual_files: ${residual.length}`)
    if (residual.length > 0) exitCode = 1
  } else {
    lines.push("next: rerun with --run to apply")
  }
  return { lines, exitCode }
}
