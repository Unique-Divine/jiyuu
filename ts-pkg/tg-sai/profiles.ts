import { mkdir, readFile, rm, writeFile } from "node:fs/promises"
import { homedir, platform as osPlatform } from "node:os"
import { dirname, join } from "node:path"
import type { UserData } from "./adminAudit"

const TG_SAI_DIR_ENV = "TG_SAI_DIR"
const CONFIG_FILE = "profiles.json"

export interface TelegramCredentials {
  apiId: number
  apiHash: string
  sessionString: string
}

export const TELEGRAM_PASSWORD_KEY = "TELEGRAM_PASSWORD" as const

export const TELEGRAM_CREDENTIAL_KEYS = [
  "TELEGRAM_API_ID",
  "TELEGRAM_API_HASH",
  "TELEGRAM_SESSION_STRING",
] as const

export type TelegramCredentialKey = (typeof TELEGRAM_CREDENTIAL_KEYS)[number]
export type TelegramProfileValueKey =
  | TelegramCredentialKey
  | typeof TELEGRAM_PASSWORD_KEY

export type TelegramCredentialValues = Partial<
  Record<TelegramProfileValueKey, string>
>

export interface TgSaiProfile {
  name: string
  apiId: number | null
  apiHash: string
  sessionString: string
  password: string
  handle: string | null
  userId: number | null
  displayName: string
  createdAt: string
  updatedAt: string
}

export interface TgSaiConfig {
  profiles: TgSaiProfile[]
}

export interface LegacyTgSaiConfig {
  activeProfile: string | null
  profiles: Record<string, TgSaiProfile>
}

export interface ProfileAddInput {
  name?: string
  apiId: number | null
  apiHash: string
  sessionString: string
  password: string
  handle: string | null
  userId: number | null
  displayName: string
}

export interface ProfileStoreEnv {
  [key: string]: string | undefined
}

export type PlatformKind = "macos" | "linux" | "windows" | "unknown"

export interface DiscoveryPath {
  name: string
  path: string
  expandedPath: string
  platform: PlatformKind
}

export interface DiscoveryCandidate {
  name: string
  path: string
  objectPath: string
  values: TelegramCredentialValues
  credentials: TelegramCredentials | null
  missingKeys: TelegramCredentialKey[]
}

export interface DiscoveryReport {
  platform: PlatformKind
  home: string
  checkedPaths: DiscoveryPath[]
  foundFiles: string[]
  candidates: DiscoveryCandidate[]
}

export const TELEGRAM_PROFILE_VALUE_KEYS = [
  ...TELEGRAM_CREDENTIAL_KEYS,
  TELEGRAM_PASSWORD_KEY,
] as const

export const MACOS_DISCOVERY_PATHS = [
  // Claude Desktop stores MCP server definitions in Application Support on macOS.
  {
    name: "Claude Desktop",
    path: "~/Library/Application Support/Claude/claude_desktop_config.json",
  },
  // Cursor stores workspace/global MCP definitions under the user's .cursor dir.
  {
    name: "Cursor",
    path: "~/.cursor/mcp.json",
  },
  // Claude Code stores account/tooling state in the user's Claude config file.
  {
    name: "Claude Code",
    path: "~/.claude.json",
  },
] as const

export const LINUX_DISCOVERY_PATHS = [
  // Cursor uses the same user-level MCP config path on Linux.
  {
    name: "Cursor",
    path: "~/.cursor/mcp.json",
  },
  // Claude Code keeps its user config in ~/.claude.json on Linux.
  {
    name: "Claude Code",
    path: "~/.claude.json",
  },
] as const

export const WINDOWS_DISCOVERY_PATHS = [
  // Cursor's Windows MCP config lives under the roaming application data tree.
  {
    name: "Cursor",
    path: "~/AppData/Roaming/Cursor/User/mcp.json",
  },
  // Keep Claude Code's user-level config as a fallback when HOME is available.
  {
    name: "Claude Code",
    path: "~/.claude.json",
  },
] as const

export const emptyConfig = (): TgSaiConfig => ({
  profiles: [],
})

export function homeDir(env: ProfileStoreEnv = process.env): string {
  return env.HOME ?? homedir()
}

export function platformKind(platform = osPlatform()): PlatformKind {
  switch (platform) {
    case "darwin":
      return "macos"
    case "linux":
      return "linux"
    case "win32":
      return "windows"
    default:
      return "unknown"
  }
}

export function discoveryPathTemplates(platform: PlatformKind) {
  switch (platform) {
    case "macos":
      return MACOS_DISCOVERY_PATHS
    case "linux":
      return LINUX_DISCOVERY_PATHS
    case "windows":
      return WINDOWS_DISCOVERY_PATHS
    default:
      return []
  }
}

export function expandHome(path: string, env: ProfileStoreEnv = process.env) {
  return path.startsWith("~/") ? join(homeDir(env), path.slice(2)) : path
}

export function tgSaiRepoDir(env: ProfileStoreEnv = process.env): string {
  return env.TG_SAI_REPO_DIR ?? import.meta.dir
}

function tgSaiEnvDiscoveryPaths(
  env: ProfileStoreEnv = process.env,
  platform = platformKind(),
): DiscoveryPath[] {
  const repoEnv = join(tgSaiRepoDir(env), ".env")
  const userEnv = join(configDir(env), ".env")
  return [
    {
      name: "tg-sai repo .env",
      path: repoEnv,
      expandedPath: repoEnv,
      platform,
    },
    {
      name: "tg-sai user .env",
      path: userEnv,
      expandedPath: userEnv,
      platform,
    },
  ]
}

export function discoveryPaths(
  env: ProfileStoreEnv = process.env,
  platform = platformKind(),
): DiscoveryPath[] {
  return [
    ...tgSaiEnvDiscoveryPaths(env, platform),
    ...discoveryPathTemplates(platform).map((candidate) => ({
      ...candidate,
      expandedPath: expandHome(candidate.path, env),
      platform,
    })),
  ]
}

/**
 * Returns the self-contained tg-sai tool directory.
 *
 * Environment variable `TG_SAI_DIR` overrides the default for tests and one-off
 * runs.
 */
export function configDir(env: ProfileStoreEnv = process.env): string {
  return env[TG_SAI_DIR_ENV] ?? join(homeDir(env), ".tg-sai")
}

export function configPath(env: ProfileStoreEnv = process.env): string {
  return join(configDir(env), CONFIG_FILE)
}

export async function readConfig(
  env: ProfileStoreEnv = process.env,
): Promise<TgSaiConfig> {
  try {
    const raw = await readFile(configPath(env), "utf8")
    const parsed = JSON.parse(raw) as TgSaiConfig | LegacyTgSaiConfig
    const config = normalizeConfig(parsed)
    for (const profile of config.profiles) {
      profile.password = profile.password ?? ""
    }
    return config
  } catch (error) {
    if (
      error instanceof Error &&
      "code" in error &&
      (error as NodeJS.ErrnoException).code === "ENOENT"
    ) {
      return emptyConfig()
    }
    throw error
  }
}

export function normalizeConfig(
  config: TgSaiConfig | LegacyTgSaiConfig,
): TgSaiConfig {
  if (Array.isArray(config.profiles)) {
    return {
      profiles: config.profiles.map((profile, index) =>
        normalizeProfile(profile, index),
      ),
    }
  }

  const legacyConfig = config as LegacyTgSaiConfig
  const legacyProfiles = Object.entries(legacyConfig.profiles)
  const orderedNames = [
    ...(legacyConfig.activeProfile &&
    legacyConfig.profiles[legacyConfig.activeProfile]
      ? [legacyConfig.activeProfile]
      : []),
    ...legacyProfiles
      .map(([name]) => name)
      .filter((name) => name !== legacyConfig.activeProfile),
  ]

  return {
    profiles: orderedNames.map((name, index) =>
      normalizeProfile({ ...legacyConfig.profiles[name]!, name }, index),
    ),
  }
}

function normalizeProfile(profile: TgSaiProfile, index: number): TgSaiProfile {
  return {
    ...profile,
    name: profile.name || profile.handle || `profile-${index}`,
    password: profile.password ?? "",
  }
}

export async function writeConfig(
  config: TgSaiConfig,
  env: ProfileStoreEnv = process.env,
): Promise<void> {
  const path = configPath(env)
  await mkdir(dirname(path), { recursive: true, mode: 0o700 })
  await writeFile(path, `${JSON.stringify(config, null, 2)}\n`, {
    mode: 0o600,
  })
}

export async function clearConfig(
  env: ProfileStoreEnv = process.env,
): Promise<void> {
  await rm(configPath(env), { force: true })
}

export function profileFromInput(
  input: ProfileAddInput,
  now = new Date(),
  index = 0,
): TgSaiProfile {
  const timestamp = now.toISOString()
  return {
    ...input,
    name: profileNameFromInput(input, index),
    createdAt: timestamp,
    updatedAt: timestamp,
  }
}

function profileNameFromInput(input: ProfileAddInput, index: number): string {
  return (
    input.name ??
    input.handle ??
    (input.userId !== null ? String(input.userId) : null) ??
    `profile-${index}`
  )
}

export async function addProfile(
  input: ProfileAddInput,
  env: ProfileStoreEnv = process.env,
  now = new Date(),
): Promise<TgSaiConfig> {
  const config = await readConfig(env)
  const index =
    input.name !== undefined
      ? findProfileIndex(config, input.name)
      : config.profiles.length > 0
        ? 0
        : -1
  const targetIndex = index >= 0 ? index : config.profiles.length
  const previous = index >= 0 ? config.profiles[index] : undefined
  const timestamp = now.toISOString()
  const nextProfile: TgSaiProfile = {
    name: profileNameFromInput(input, targetIndex),
    apiId: input.apiId ?? previous?.apiId ?? null,
    apiHash: input.apiHash || previous?.apiHash || "",
    sessionString: input.sessionString || previous?.sessionString || "",
    password: input.password || previous?.password || "",
    handle: input.handle ?? previous?.handle ?? null,
    userId: input.userId ?? previous?.userId ?? null,
    displayName: input.displayName || previous?.displayName || "",
    createdAt: previous?.createdAt ?? timestamp,
    updatedAt: timestamp,
  }

  if (index >= 0) {
    config.profiles[index] = nextProfile
  } else {
    config.profiles.push(nextProfile)
  }

  await writeConfig(config, env)
  return config
}

export async function removeProfile(
  selector: string,
  env: ProfileStoreEnv = process.env,
): Promise<TgSaiConfig> {
  const config = await readConfig(env)
  const index = findProfileIndex(config, selector)
  if (index === -1) {
    throw new Error(`Unknown profile: ${selector}`)
  }
  config.profiles.splice(index, 1)

  await writeConfig(config, env)
  return config
}

export async function setActiveProfile(
  selector: string,
  env: ProfileStoreEnv = process.env,
): Promise<TgSaiConfig> {
  const config = await readConfig(env)
  const index = findProfileIndex(config, selector)
  if (index === -1) {
    throw new Error(`Unknown profile: ${selector}`)
  }
  const [profile] = config.profiles.splice(index, 1)
  config.profiles.unshift(profile!)
  await writeConfig(config, env)
  return config
}

export function getProfile(
  config: TgSaiConfig,
  selector: string | undefined,
): TgSaiProfile | null {
  if (selector === undefined) {
    return config.profiles[0] ?? null
  }
  const index = findProfileIndex(config, selector)
  return index === -1 ? null : config.profiles[index]!
}

export function findProfileIndex(
  config: TgSaiConfig,
  selector: string,
): number {
  const asIndex = Number(selector)
  if (Number.isInteger(asIndex) && asIndex >= 0) {
    return asIndex < config.profiles.length ? asIndex : -1
  }
  return config.profiles.findIndex(
    (profile) =>
      profile.name === selector ||
      profile.handle === selector ||
      String(profile.userId) === selector,
  )
}

export function profileLabel(profile: TgSaiProfile, index: number): string {
  return profile.handle ?? profile.name ?? `profile-${index}`
}

export function redactedProfile(profile: TgSaiProfile) {
  return {
    name: profile.name,
    handle: profile.handle,
    userId: profile.userId,
    displayName: profile.displayName,
    apiId: profile.apiId,
    apiHash: profile.apiHash ? "[redacted]" : "",
    sessionString: profile.sessionString ? "[redacted]" : "",
    password: profile.password ? "[redacted]" : "",
    createdAt: profile.createdAt,
    updatedAt: profile.updatedAt,
  }
}

export function credentialsFromProfileRecord(
  profile: TgSaiProfile,
): TelegramCredentials | null {
  if (!profile.apiId || !profile.apiHash || !profile.sessionString) {
    return null
  }
  return {
    apiId: profile.apiId,
    apiHash: profile.apiHash,
    sessionString: profile.sessionString,
  }
}

export function credentialsFromEnv(
  env: ProfileStoreEnv = process.env,
): TelegramCredentials | null {
  const apiId = env.TELEGRAM_API_ID
  const apiHash = env.TELEGRAM_API_HASH
  const sessionString = env.TELEGRAM_SESSION_STRING
  if (!apiId || !apiHash || !sessionString) {
    return null
  }
  return {
    apiId: Number(apiId),
    apiHash,
    sessionString,
  }
}

function unquote(value: string): string {
  return value.trim().replace(/^['"]|['"]$/g, "")
}

export function credentialsFromText(text: string): TelegramCredentials | null {
  const values = telegramCredentialValuesFromLines(text)
  const apiId = values.TELEGRAM_API_ID
  const apiHash = values.TELEGRAM_API_HASH
  const sessionString = values.TELEGRAM_SESSION_STRING
  if (!apiId || !apiHash || !sessionString) {
    return null
  }

  return {
    apiId: Number(apiId),
    apiHash,
    sessionString,
  }
}

export function parseCredentialValueAfterKey(afterKey: string): string | null {
  const trimmed = afterKey.trimStart()
  const withoutSeparator = trimmed.replace(/^["']?\s*[:=]\s*/, "").trimStart()
  if (withoutSeparator.length === 0) {
    return null
  }

  const quote = withoutSeparator[0]
  if (quote === '"' || quote === "'") {
    let value = ""
    for (let index = 1; index < withoutSeparator.length; index += 1) {
      const char = withoutSeparator[index]
      if (char === quote && withoutSeparator[index - 1] !== "\\") {
        return value
      }
      value += char
    }
    return value.length > 0 ? value : null
  }

  const unquoted = withoutSeparator.split(/[,\s#]/, 1)[0]?.trim()
  return unquoted && unquoted.length > 0 ? unquoted : null
}

export function parseCredentialLine(line: string): TelegramCredentialValues {
  for (const key of TELEGRAM_PROFILE_VALUE_KEYS) {
    const index = line.indexOf(key)
    if (index === -1) {
      continue
    }

    const afterKey = line.slice(index + key.length)
    const value = parseCredentialValueAfterKey(afterKey)
    return value ? { [key]: value } : {}
  }
  return {}
}

export function telegramCredentialValuesFromLines(
  text: string,
): TelegramCredentialValues {
  return text
    .split(/\r?\n/)
    .map(parseCredentialLine)
    .reduce<TelegramCredentialValues>(
      (acc, values) => ({ ...acc, ...values }),
      {},
    )
}

function credentialValuesFromObject(value: unknown): TelegramCredentialValues {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return {}
  }

  const record = value as Record<string, unknown>
  const hasCandidateKey = TELEGRAM_PROFILE_VALUE_KEYS.some(
    (key) => key in record,
  )
  if (!hasCandidateKey) {
    return {}
  }

  return TELEGRAM_PROFILE_VALUE_KEYS.reduce<TelegramCredentialValues>(
    (acc, key) => {
      const valueForKey = record[key]
      if (typeof valueForKey === "string" || typeof valueForKey === "number") {
        acc[key] = String(valueForKey)
      }
      return acc
    },
    {},
  )
}

function credentialsFromCredentialValues(
  values: TelegramCredentialValues,
): TelegramCredentials | null {
  const apiId = values.TELEGRAM_API_ID
  const apiHash = values.TELEGRAM_API_HASH
  const sessionString = values.TELEGRAM_SESSION_STRING
  if (!apiId || !apiHash || !sessionString) {
    return null
  }

  return {
    apiId: Number(apiId),
    apiHash,
    sessionString,
  }
}

function missingCredentialKeys(
  values: TelegramCredentialValues,
): TelegramCredentialKey[] {
  return TELEGRAM_CREDENTIAL_KEYS.filter((key) => !values[key])
}

export function findTelegramCredentialBlocks(
  value: unknown,
  objectPath = "$",
): Array<{
  objectPath: string
  values: TelegramCredentialValues
  credentials: TelegramCredentials | null
  missingKeys: TelegramCredentialKey[]
}> {
  const values = credentialValuesFromObject(value)
  const hasValues = Object.keys(values).length > 0
  const found = hasValues
    ? [
        {
          objectPath,
          values,
          credentials: credentialsFromCredentialValues(values),
          missingKeys: missingCredentialKeys(values),
        },
      ]
    : []

  if (typeof value !== "object" || value === null) {
    return found
  }

  if (Array.isArray(value)) {
    value.forEach((item, index) => {
      found.push(
        ...findTelegramCredentialBlocks(item, `${objectPath}[${index}]`),
      )
    })
    return found
  }

  for (const [key, child] of Object.entries(value)) {
    found.push(...findTelegramCredentialBlocks(child, `${objectPath}.${key}`))
  }
  return found
}

export async function discoveryReport(
  env: ProfileStoreEnv = process.env,
  platform = platformKind(),
): Promise<DiscoveryReport> {
  const checkedPaths = discoveryPaths(env, platform)
  const foundFiles: string[] = []
  const candidates: DiscoveryCandidate[] = []

  for (const candidatePath of checkedPaths) {
    try {
      const text = await readFile(candidatePath.expandedPath, "utf8")
      foundFiles.push(candidatePath.expandedPath)
      let blocks: Array<{
        objectPath: string
        values: TelegramCredentialValues
        credentials: TelegramCredentials | null
        missingKeys: TelegramCredentialKey[]
      }> = []
      try {
        const parsed = JSON.parse(text) as unknown
        blocks = findTelegramCredentialBlocks(parsed)
      } catch (error) {
        if (!(error instanceof SyntaxError)) {
          throw error
        }
      }
      if (blocks.length === 0) {
        const values = telegramCredentialValuesFromLines(text)
        if (Object.keys(values).length > 0) {
          blocks = [
            {
              objectPath: "$.lineScan",
              values,
              credentials: credentialsFromCredentialValues(values),
              missingKeys: missingCredentialKeys(values),
            },
          ]
        }
      }
      for (const block of blocks) {
        candidates.push({
          name: candidatePath.name,
          path: candidatePath.expandedPath,
          objectPath: block.objectPath,
          values: block.values,
          credentials: block.credentials,
          missingKeys: block.missingKeys,
        })
      }
    } catch (error) {
      if (
        error instanceof Error &&
        "code" in error &&
        (error as NodeJS.ErrnoException).code === "ENOENT"
      ) {
        continue
      }
      throw error
    }
  }
  return {
    platform,
    home: homeDir(env),
    checkedPaths,
    foundFiles,
    candidates,
  }
}

export async function credentialsFromKnownFiles(
  env: ProfileStoreEnv = process.env,
): Promise<TelegramCredentials | null> {
  if (env.TG_SAI_DISCOVERY_FILE) {
    const text = await readFile(env.TG_SAI_DISCOVERY_FILE, "utf8")
    const textCredentials = credentialsFromText(text)
    if (textCredentials) {
      return textCredentials
    }
    try {
      const parsed = JSON.parse(text) as unknown
      const complete = findTelegramCredentialBlocks(parsed).find(
        (candidate) => candidate.credentials !== null,
      )
      return complete?.credentials ?? null
    } catch (error) {
      if (error instanceof SyntaxError) {
        return null
      }
      throw error
    }
  }

  const report = await discoveryReport(env)
  const completeCandidates = report.candidates.filter(
    (candidate) => candidate.credentials !== null,
  )
  if (completeCandidates.length === 1) {
    return completeCandidates[0]!.credentials
  }
  if (completeCandidates.length > 1) {
    throw new Error(
      `Multiple Telegram credential sources found: ${completeCandidates
        .map((candidate) => `${candidate.name} ${candidate.objectPath}`)
        .join(", ")}`,
    )
  }
  return null
}

export async function discoverTelegramCredentials(
  env: ProfileStoreEnv = process.env,
): Promise<TelegramCredentials | null> {
  return credentialsFromEnv(env) ?? (await credentialsFromKnownFiles(env))
}

export async function credentialsFromProfile(
  profileName: string | undefined,
  env: ProfileStoreEnv = process.env,
): Promise<TelegramCredentials | null> {
  const config = await readConfig(env)
  const profile = getProfile(config, profileName)
  if (!profile) {
    return null
  }
  return credentialsFromProfileRecord(profile)
}

export async function resolveTelegramCredentials(
  profileName: string | undefined,
  env: ProfileStoreEnv = process.env,
): Promise<TelegramCredentials> {
  const envCredentials = credentialsFromEnv(env)
  if (envCredentials) {
    return envCredentials
  }

  const config = await readConfig(env)
  const selectedProfile = getProfile(config, profileName)
  const profileCredentials = selectedProfile
    ? credentialsFromProfileRecord(selectedProfile)
    : null
  if (profileCredentials) {
    return profileCredentials
  }
  if (selectedProfile) {
    throw new Error(
      `Profile ${selectedProfile.name} is incomplete. Fill apiId, apiHash, and sessionString before running Telegram operations.`,
    )
  }

  const knownFileCredentials = await credentialsFromKnownFiles(env)
  if (knownFileCredentials) {
    return knownFileCredentials
  }

  throw new Error(
    "Missing Telegram credentials. Set TELEGRAM_API_ID, TELEGRAM_API_HASH, and TELEGRAM_SESSION_STRING or run `tg-sai profile add`.",
  )
}

export function profileInputFromUser(
  name: string | undefined,
  credentials: TelegramCredentials,
  user: UserData,
  password = "",
): ProfileAddInput {
  return {
    name,
    apiId: credentials.apiId,
    apiHash: credentials.apiHash,
    sessionString: credentials.sessionString,
    password,
    handle: user.username,
    userId: user.id,
    displayName: user.displayName,
  }
}
