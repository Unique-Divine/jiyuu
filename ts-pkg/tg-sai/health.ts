import { constants } from "node:fs"
import { access, mkdir, readdir, readFile, stat } from "node:fs/promises"
import { join } from "node:path"

import { configDir, configPath, readConfig } from "./profiles"
import type {
  ProfileStoreEnv,
  TelegramCredentials,
  TgSaiProfile,
} from "./profiles"

const ERROR_PREFIX = "❌😭"
const WARNING_PREFIX = "⚠️"
const GOOD_PREFIX = "✅🔥"

export interface HealthUser {
  id: number
  username: string | null
  displayName: string
  isBot: boolean
}

export interface HealthClient {
  getMe(): Promise<HealthUser>
  disconnect(): Promise<void>
}

export type HealthClientFactory = (
  credentials: TelegramCredentials,
) => Promise<HealthClient>

export interface HealthResult {
  exitCode: 0 | 1 | 2
  lines: string[]
  errorCount: number
  warningCount: number
}

interface HealthCollector {
  lines: string[]
  problems: HealthProblem[]
  errorCount: number
  warningCount: number
}

interface HealthProblem {
  key: string
  severity: "error" | "warning"
  fix: string[]
}

function createCollector(): HealthCollector {
  return {
    lines: [],
    problems: [],
    errorCount: 0,
    warningCount: 0,
  }
}

function info(collector: HealthCollector, line: string): void {
  collector.lines.push(line)
}

function good(collector: HealthCollector, line: string): void {
  collector.lines.push(`${GOOD_PREFIX} ${line}`)
}

function warning(
  collector: HealthCollector,
  key: string,
  line: string,
  fix: string[],
): void {
  collector.warningCount += 1
  collector.lines.push(`${WARNING_PREFIX} ${line}`)
  collector.problems.push({ key, severity: "warning", fix })
}

function error(
  collector: HealthCollector,
  key: string,
  line: string,
  fix: string[],
): void {
  collector.errorCount += 1
  collector.lines.push(`${ERROR_PREFIX} ${line}`)
  collector.problems.push({ key, severity: "error", fix })
}

function exitCode(collector: HealthCollector): 0 | 1 | 2 {
  if (collector.errorCount > 0) {
    return 1
  }
  if (collector.warningCount > 0) {
    return 2
  }
  return 0
}

function errorMessage(value: unknown): string {
  return value instanceof Error ? value.message : String(value)
}

function directoryFix(path: string): string[] {
  return [
    `Make sure the directory exists and is writable: ${path}`,
    `mkdir -p ${path}`,
    `chmod 700 ${path}`,
  ]
}

function missingProfileFix(profileName?: string): string[] {
  const addCommand = profileName
    ? `tg-sai profile add --name ${profileName} --src <number>`
    : "tg-sai profile add --name <name> --src <number>"
  return [
    "Find a Telegram credential source and save it as a tg-sai profile.",
    "tg-sai profile find",
    addCommand,
  ]
}

function incompleteProfileFix(profileName: string, fields: string[]): string[] {
  return [
    `Fill missing profile field(s): ${fields.join(",")}`,
    "Check the redacted active profile:",
    `tg-sai profile show ${profileName}`,
    "Then refresh it from a complete credential source:",
    "tg-sai profile find",
    `tg-sai profile add --name ${profileName} --src <number>`,
  ]
}

function connectionFix(profileName: string): string[] {
  return [
    "Check the active profile API credentials and Telegram session string.",
    `tg-sai profile show ${profileName}`,
    "If apiId, apiHash, or sessionString are stale, refresh from a known-good source:",
    "tg-sai profile find",
    `tg-sai profile add --name ${profileName} --src <number>`,
  ]
}

function passwordFix(profileName?: string): string[] {
  const saveCommand = profileName
    ? `tg-sai profile add --name ${profileName} --password '<password>'`
    : "tg-sai profile add --name <name> --password '<password>'"
  return [
    "Save the Telegram 2FA password in the active tg-sai profile:",
    saveCommand,
    "You can also override it for one command with TELEGRAM_PASSWORD.",
  ]
}

function renderSuggestedFixes(collector: HealthCollector): void {
  if (collector.problems.length === 0) {
    return
  }

  info(collector, "")
  info(collector, "suggested_fixes:")
  for (const problem of collector.problems) {
    info(collector, `- ${problem.key} (${problem.severity}):`)
    for (const line of problem.fix) {
      info(collector, `  ${line}`)
    }
  }
}

async function ensureWritableDirectory(
  collector: HealthCollector,
  label: string,
  path: string,
): Promise<boolean> {
  try {
    await mkdir(path, { recursive: true, mode: 0o700 })
    await access(path, constants.W_OK)
    info(collector, `${label}: ${path}`)
    return true
  } catch (caught) {
    error(
      collector,
      label,
      `${label}: ${path} not_writable ${errorMessage(caught)}`,
      directoryFix(path),
    )
    return false
  }
}

async function profileFileExists(env: ProfileStoreEnv): Promise<boolean> {
  try {
    const fileStat = await stat(configPath(env))
    return fileStat.isFile()
  } catch (caught) {
    if (
      caught instanceof Error &&
      "code" in caught &&
      (caught as NodeJS.ErrnoException).code === "ENOENT"
    ) {
      return false
    }
    throw caught
  }
}

export function missingProfileFields(profile: TgSaiProfile): string[] {
  const missing: string[] = []
  if (!profile.apiId) {
    missing.push("apiId")
  }
  if (!profile.apiHash) {
    missing.push("apiHash")
  }
  if (!profile.sessionString) {
    missing.push("sessionString")
  }
  return missing
}

function credentialsFromCompleteProfile(
  profile: TgSaiProfile,
): TelegramCredentials {
  return {
    apiId: profile.apiId ?? 0,
    apiHash: profile.apiHash,
    sessionString: profile.sessionString,
  }
}

async function checkTelegramConnection(params: {
  collector: HealthCollector
  profile: TgSaiProfile | null
  missingFields: string[]
  createClient: HealthClientFactory
}): Promise<void> {
  const { collector, profile, missingFields, createClient } = params
  if (!profile) {
    error(
      collector,
      "telegram_connection",
      "telegram_connection: skipped because no active profile is configured",
      missingProfileFix(),
    )
    return
  }
  if (missingFields.length > 0) {
    error(
      collector,
      "telegram_connection",
      "telegram_connection: skipped because active profile is incomplete",
      incompleteProfileFix(profile.name, missingFields),
    )
    return
  }

  let client: HealthClient | null = null
  try {
    client = await createClient(credentialsFromCompleteProfile(profile))
    const user = await client.getMe()
    good(collector, "telegram_connection: ok")
    info(
      collector,
      `session_user: ${user.id} ${user.username ?? ""} "${user.displayName}"`,
    )
    if (user.isBot) {
      warning(
        collector,
        "session_user_is_bot",
        "session_user_is_bot: true; tg-sai is designed for user-account sessions",
        [
          "Use a Telegram user-account session for tg-sai admin operations.",
          "Switch to a user profile:",
          "tg-sai profile list",
          "tg-sai profile use <user-profile-name>",
        ],
      )
    }
  } catch (caught) {
    error(
      collector,
      "telegram_connection",
      `telegram_connection: failed ${errorMessage(caught)}`,
      connectionFix(profile.name),
    )
  } finally {
    if (client) {
      await client.disconnect()
    }
  }
}

function checkPassword(
  collector: HealthCollector,
  env: ProfileStoreEnv,
  profile: TgSaiProfile | null,
): void {
  if (env.TELEGRAM_PASSWORD || profile?.password) {
    info(collector, "password_status: present")
    return
  }
  warning(
    collector,
    "password_status",
    "password_status: missing",
    passwordFix(profile?.name),
  )
  info(
    collector,
    "No profile password or TELEGRAM_PASSWORD is set; ownership transfer will be skipped",
  )
}

function latestNumberedJsonl(files: string[], prefix: string): string | null {
  const matcher = new RegExp(`^${prefix}-(\\d+)\\.jsonl$`)
  let latest: { file: string; index: number } | null = null
  for (const file of files) {
    const match = file.match(matcher)
    if (!match) {
      continue
    }
    const index = Number(match[1])
    if (!latest || index > latest.index) {
      latest = { file, index }
    }
  }
  return latest?.file ?? null
}

async function countJsonlRecords(path: string): Promise<number> {
  const text = await readFile(path, "utf8")
  return text.split(/\r?\n/).filter((line) => line.trim().length > 0).length
}

async function checkLogs(
  collector: HealthCollector,
  logsPath: string,
): Promise<void> {
  let files: string[] = []
  try {
    files = await readdir(logsPath)
  } catch (caught) {
    error(
      collector,
      "logs_scan",
      `logs_scan: failed ${errorMessage(caught)}`,
      directoryFix(logsPath),
    )
    return
  }

  const saiFind = latestNumberedJsonl(files, "sai-find")
  if (saiFind) {
    const path = join(logsPath, saiFind)
    info(collector, `latest_sai_find: ${path}`)
    info(
      collector,
      `latest_sai_find_chat_count: ${await countJsonlRecords(path)}`,
    )
  } else {
    info(collector, "latest_sai_find: none")
    info(collector, "next: tg-sai ops sai-find")
  }

  const saiBootstrap = latestNumberedJsonl(files, "sai-bootstrap")
  if (saiBootstrap) {
    info(collector, `latest_sai_bootstrap: ${join(logsPath, saiBootstrap)}`)
  } else {
    info(collector, "latest_sai_bootstrap: none")
  }
}

export async function runHealthCheck(params: {
  env?: ProfileStoreEnv
  createClient: HealthClientFactory
}): Promise<HealthResult> {
  const env = params.env ?? process.env
  const collector = createCollector()
  const toolPath = configDir(env)
  const logsPath = join(toolPath, "logs")

  await ensureWritableDirectory(collector, "tool_dir", toolPath)
  const logsWritable = await ensureWritableDirectory(
    collector,
    "logs_dir",
    logsPath,
  )

  if (await profileFileExists(env)) {
    info(collector, `profile_file: ${configPath(env)}`)
  } else {
    info(collector, `profile_file: missing ${configPath(env)}`)
  }

  const config = await readConfig(env)
  const activeProfileName = config.activeProfile
  const activeProfile = activeProfileName
    ? (config.profiles[activeProfileName] ?? null)
    : null
  const missingFields = activeProfile ? missingProfileFields(activeProfile) : []

  if (!activeProfileName) {
    error(
      collector,
      "active_profile",
      "active_profile: missing",
      missingProfileFix(),
    )
    error(
      collector,
      "profile_status",
      "profile_status: missing",
      missingProfileFix(),
    )
  } else if (!activeProfile) {
    error(
      collector,
      "active_profile",
      `active_profile: ${activeProfileName}`,
      missingProfileFix(activeProfileName),
    )
    error(
      collector,
      "profile_status",
      "profile_status: missing",
      missingProfileFix(activeProfileName),
    )
  } else {
    info(collector, `active_profile: ${activeProfile.name}`)
    if (missingFields.length > 0) {
      error(
        collector,
        "profile_status",
        "profile_status: incomplete",
        incompleteProfileFix(activeProfile.name, missingFields),
      )
      error(
        collector,
        "missing_fields",
        `missing_fields: ${missingFields.join(",")}`,
        incompleteProfileFix(activeProfile.name, missingFields),
      )
    } else {
      good(collector, "profile_status: complete")
    }
  }

  await checkTelegramConnection({
    collector,
    profile: activeProfile,
    missingFields,
    createClient: params.createClient,
  })
  checkPassword(collector, env, activeProfile)
  if (logsWritable) {
    await checkLogs(collector, logsPath)
  }

  if (!activeProfileName) {
    info(collector, "next: tg-sai profile find")
    info(collector, "next: tg-sai profile add --name <name> --src <number>")
  } else if (!activeProfile || missingFields.length > 0) {
    info(
      collector,
      `next: tg-sai profile add --name ${activeProfileName} --src <number>`,
    )
  }

  renderSuggestedFixes(collector)
  const code = exitCode(collector)
  info(collector, `exit_code: ${code}`)
  return {
    exitCode: code,
    lines: collector.lines,
    errorCount: collector.errorCount,
    warningCount: collector.warningCount,
  }
}
