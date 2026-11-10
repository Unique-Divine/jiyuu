#!/usr/bin/env bun
import {
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { stdin as input, stdout as output } from "node:process"
import { createInterface } from "node:readline/promises"
import { Command, Option } from "commander"
import { addProvisionCommands } from "./provisioningCli"
import { addCustodyWorkerCommands } from "./custodyWorkerCli"
import type { BatchLogger } from "./actionExecutor"
import { runChatBatch, runTelegramAction } from "./actionExecutor"
import type {
  ChatAuditInput,
  ChatMemberData,
  PromoteOnlyAction,
  ProposedAction,
} from "./adminAudit"
import { planAdminBootstrap, planPromoteOnly } from "./adminAudit"
import { saiBot, USERS } from "./cfg"
import { runHealthCheck } from "./health"
import { addRuntimeHealthCommands } from "./runtimeHealthCli"
import { HOLD_ACCOUNT_UNKNOWN, holdErrorSeconds } from "./state/guard"
import { stateDbPath } from "./state/db"
import { registerEventsCommand } from "./telemetry/cli"
import { registerHoldsCommand } from "./state/cli"
import type { TelemetrySubsystem } from "./telemetry/db"
import {
  activeTelemetryRun,
  BOOTSTRAP_FLOOD_MAX_WAIT_MS,
  floodMaxWaitMs,
  startTelemetryRun,
} from "./telemetry/run"
import {
  addTargetToChat,
  createMtcuteClient,
  fetchChatAuditInput,
  findSaiChats,
  normalizeUser,
  fetchChatTitle,
  promoteExistingMember,
  promoteTargetAdmin,
  transferTargetOwnership,
} from "./mtcuteAdapter"
import {
  formatSaiChatJsonLine,
  parseChatInputFile,
  resolveBootstrapTarget,
} from "./ops"
import type { ChatInput } from "./ops"
import {
  addProfile,
  configDir,
  discoveryReport,
  findProfileIndex,
  getProfile,
  profileInputFromUser,
  profileLabel,
  readConfig,
  redactedProfile,
  plainTextPasswordProfiles,
  profilePasswordStorage,
  removeProfile,
  resolveTelegramCredentialSource,
  setActiveProfile,
} from "./profiles"
import {
  assertPassReady,
  defaultPasswordPassEntry,
  insertPassPassword,
  passEntryExists,
  promptHiddenPassword,
  readPasswordFromStdin,
  readTelegramPassword,
  validatePassEntry,
} from "./passwordStore"
import type { TelegramPasswordRef } from "./passwordStore"
import { migrateProfilePasswords } from "./passwordMigration"
import type { MigratePasswordOptions } from "./passwordMigration"
import type {
  DiscoveryReport,
  ProfileAddInput,
  ProfileStoreEnv,
  TelegramCredentials,
  TelegramCredentialValues,
} from "./profiles"
// Static import so `bun build --compile` embeds the version in the worker.
import packageJson from "./package.json" with { type: "json" }

const DEFAULT_USER_TARGET_ID = USERS.uniqueDivine.id
const DEFAULT_BOT_TARGET_ID = saiBot.id
const DEFAULT_BOT_TARGET_PEER = saiBot.handle

interface BaseCliOptions {
  chat: number | string
  targetUser: number | string
  profile?: string
}

interface AddBotOptions extends BaseCliOptions {
  targetPeer: number | string
  run?: boolean
}

interface AddOwnerOptions {
  chat: number | string
  targetUser: number | string
  profile?: string
  run?: boolean
}

/**
 * Parsed options for command `tg-sai promote-admin`.
 */
interface PromoteAdminOptions {
  chat: number | string
  /** Existing chat member to promote: user ID or username. */
  targetUser: number | string
  profile?: string
  /** Execute the promotion when true; default behavior is dry-run. */
  run?: boolean
  /** Allow a target outside the team allowlist (`promoteAdminTeamIds`). */
  allowNonTeam?: boolean
}

/**
 * User IDs command `tg-sai promote-admin` may promote without flag
 * `--allow-non-team`: the team accounts in `cfg.ts` `USERS`.
 */
export const promoteAdminTeamIds: ReadonlySet<number> = new Set(
  Object.values(USERS).map((user) => user.id),
)

/**
 * Parsed options for command `tg-sai ops sai-find`.
 */
interface SaiFindOptions {
  /** Optional local Telegram credential profile name. */
  profile?: string
  /** Maximum number of dialogs to inspect before local filtering. */
  limit: string
  /** Optional extra output path for a copy of the JSONL chat records. */
  out?: string
  /** Archive scope for dialog discovery. */
  archived?: "include" | "exclude" | "only"
}

/**
 * Parsed options for command `tg-sai ops sai-bootstrap`.
 */
interface SaiBootstrapOptions {
  /** Optional local Telegram credential profile name. */
  profile?: string
  /** Path to a reviewed chat input file. */
  file: string
  /** Inter-chat delay in milliseconds. */
  delayMs: string
  /** Execute writes when true; default behavior is dry-run planning. */
  run?: boolean
  /** Attempt ownership transfer when the Telegram password is available. */
  transferOwner?: boolean
  /** Owner target: user ID or username. Defaults to the Sai custody account. */
  targetUser?: number | string
}

export interface TelegramEnv {
  apiId: number
  apiHash: string
  session: string
  /** Where to read the 2FA password; read only for ownership transfer. */
  password: TelegramPasswordRef | null
  /** Saved account user ID, only for a session from a saved profile. */
  userId: number | null
  /** Saved profile name, only for a session from that profile. */
  profileName: string | null
}

interface ProfileAddOptions {
  name?: string
  apiId?: string
  apiHash?: string
  sessionString?: string
  /** Removed flag; kept so the CLI can point to its replacements. */
  password?: string
  passwordPassEntry?: string
  passwordStdin?: boolean
  passwordPrompt?: boolean
  src?: string
  handle?: string
  userId?: string
  displayName?: string
  login?: boolean
  qr?: boolean
  phone?: boolean
  interactive?: boolean
}

const saiBotAdminRights = {
  changeInfo: false,
  postMessages: false,
  editMessages: false,
  deleteMessages: true,
  banUsers: true,
  inviteUsers: true,
  pinMessages: true,
  addAdmins: false,
  anonymous: false,
  manageCall: false,
  other: false,
  manageTopics: true,
  postStories: false,
  editStories: false,
  deleteStories: false,
  manageDirectMessages: false,
  manageRanks: false,
}

const ownerTargetAdminRights = {
  changeInfo: true,
  postMessages: false,
  editMessages: false,
  deleteMessages: true,
  banUsers: true,
  inviteUsers: true,
  pinMessages: true,
  addAdmins: true,
  anonymous: false,
  manageCall: true,
  other: true,
  manageTopics: true,
  postStories: false,
  editStories: false,
  deleteStories: false,
  manageDirectMessages: false,
  manageRanks: true,
}

const DEFAULT_INTER_CHAT_DELAY_MS = 1000

/**
 * Emits one structured JSON object to stdout. Durable records live in the
 * telemetry DB (`tg-sai events`); stdout is for the operator.
 */
function emitJsonObject(fields: Record<string, unknown>): void {
  const object = { ts: new Date().toISOString(), ...fields }
  console.log(JSON.stringify(object))
}

/**
 * Batch logger that emits each structured event as one JSON object on stdout.
 *
 * Operators can redirect stdout to a `.jsonl` file when they want a durable run
 * log, while tests can use `MemoryBatchLogger` from file `actionExecutor.ts`.
 */
const consoleBatchLogger: BatchLogger = {
  event(event) {
    emitJsonObject(event)
  },
}

/**
 * Promise-based sleep used by the Telegram action executor.
 */
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/**
 * Progress-logging dependencies for live Telegram actions. Flood waits are
 * handled by mtcute's flood waiter, not here.
 */
function actionExecutorDeps(logger: BatchLogger = consoleBatchLogger) {
  return {
    logger,
    now: () => Date.now(),
  }
}

function actorBatchLogger(actorUsername: string): BatchLogger {
  return {
    event(event) {
      emitJsonObject({
        ...event,
        actorUsername,
      })
    },
  }
}

/**
 * Emits one command-level JSON event to stdout.
 */
function printJsonEvent(type: string, fields: Record<string, unknown>): void {
  emitJsonObject({ type, ...fields })
}

/**
 * Directory for `ops sai-find` chat files. These are command output (chat
 * lists for `ops sai-bootstrap --file`), not logs; logs live in the
 * telemetry DB.
 */
function chatFilesDir(): string {
  return join(configDir(), "logs")
}

/**
 * Chooses the next numbered chat file path for a named command prefix.
 *
 * Example output paths are `sai-find-00.jsonl`, `sai-find-01.jsonl`, and so on.
 */
async function nextChatFilePath(prefix: string): Promise<string> {
  const dir = chatFilesDir()
  await mkdir(dir, { recursive: true, mode: 0o700 })

  const files = await readdir(dir).catch((error) => {
    if (
      error instanceof Error &&
      "code" in error &&
      (error as NodeJS.ErrnoException).code === "ENOENT"
    ) {
      return []
    }
    throw error
  })
  const matcher = new RegExp(`^${prefix}-(\\d+)\\.jsonl$`)
  const maxIndex = files.reduce((max, file) => {
    const match = file.match(matcher)
    if (!match) {
      return max
    }
    return Math.max(max, Number(match[1]))
  }, -1)
  const nextIndex = maxIndex + 1
  const suffix = String(nextIndex).padStart(2, "0")
  return join(dir, `${prefix}-${suffix}.jsonl`)
}

/**
 * Session settings for one command. The password, account ID, and profile
 * label all come from the same source as the session, so a `.env` or MCP
 * session never pairs with a saved profile's 2FA password or name.
 */
export async function getTelegramEnv(
  profileName?: string,
  env: ProfileStoreEnv = process.env,
): Promise<TelegramEnv> {
  const resolved = await resolveTelegramCredentialSource(profileName, env)
  return {
    apiId: resolved.credentials.apiId,
    apiHash: resolved.credentials.apiHash,
    session: resolved.credentials.sessionString,
    password: resolved.password,
    userId: resolved.userId,
    profileName: resolved.profileName,
  }
}

/** Telegram telemetry for the current CLI run, if one was started. */
function cliTelemetry(identity: {
  accountId?: number | null
  profile?: string | null
} = {}) {
  return activeTelemetryRun()?.client(identity)
}

async function selectedProfileName(
  profileName?: string,
): Promise<string | null> {
  if (profileName) {
    return profileName
  }
  const config = await readConfig()
  const profile = config.profiles[0]
  return profile ? profileLabel(profile, 0) : null
}

function parsePeer(value: string): number | string {
  const normalized = value.startsWith("@") ? value.slice(1) : value
  const asNumber = Number(normalized)
  return Number.isFinite(asNumber) ? asNumber : normalized
}

function showMember(label: string, member: ChatMemberData | null) {
  if (!member) {
    console.log(`${label}: not_present`)
    return
  }

  console.log(`${label}:`)
  console.log(`  user_id: ${member.user.id}`)
  console.log(`  username: ${member.user.username ?? ""}`)
  console.log(`  status: ${member.status}`)
  console.log(`  is_admin: ${member.status === "admin"}`)
  console.log(`  is_creator: ${member.status === "creator"}`)
  console.log(`  has_admin_permissions: ${member.permissions !== null}`)
  if (member.permissions) {
    console.log(`  admin_permissions: ${JSON.stringify(member.permissions)}`)
  }
}

function printAudit(input: ChatAuditInput) {
  const audit = planAdminBootstrap(input)

  console.log("connection_ok")
  console.log(`session_user: ${input.self.id} ${input.self.username ?? ""}`)
  console.log(`chat_id: ${input.chatId}`)
  console.log(`target_user_id: ${input.targetUserId}`)

  showMember("actor", input.actor)
  showMember("target", input.target)
  console.log(`actor_can_apply: ${audit.actorCanApply}`)
  console.log(
    `actor_can_transfer_ownership: ${audit.actorCanTransferOwnership}`,
  )
  console.log(
    `target_can_receive_ownership: ${audit.targetCanReceiveOwnership}`,
  )
  console.log(`proposed_actions: ${formatActionList(audit.proposedActions)}`)
}

function shouldRunAction(
  proposedActions: ProposedAction[],
  action: ProposedAction,
): boolean {
  return proposedActions.includes(action)
}

function formatActionList(
  actions: readonly (ProposedAction | PromoteOnlyAction)[],
): string {
  return JSON.stringify(actions)
}

async function withClient<T>(
  profileName: string | undefined,
  fn: (
    client: Awaited<ReturnType<typeof createMtcuteClient>>,
    env: TelegramEnv,
  ) => Promise<T>,
): Promise<T> {
  const env = await getTelegramEnv(profileName)
  // No `accountId`: the flood guard keys holds by the account this session's
  // own getMe reports, never by profile metadata.
  const client = await createMtcuteClient(
    {
      apiId: env.apiId,
      apiHash: env.apiHash,
      session: env.session,
    },
    cliTelemetry({ accountId: env.userId, profile: env.profileName }),
  )

  try {
    return await fn(client, env)
  } finally {
    await client.disconnect()
  }
}

async function runAudit(options: BaseCliOptions): Promise<void> {
  await withClient(options.profile, async (client) => {
    const auditInput = await fetchChatAuditInput(client, {
      chatId: options.chat,
      targetUserId: options.targetUser,
    })
    printAudit(auditInput)
  })
}

async function runHealth(): Promise<void> {
  // runHealthCheck connects as the active (first) profile. Its saved
  // userId only labels telemetry; the flood guard keys by the session's getMe.
  const accountId = (await readConfig()).profiles[0]?.userId ?? null
  const result = await runHealthCheck({
    createClient: async (credentials) =>
      createMtcuteClient(
        {
          apiId: credentials.apiId,
          apiHash: credentials.apiHash,
          session: credentials.sessionString,
        },
        cliTelemetry({ accountId }),
      ),
  })
  for (const line of result.lines) {
    console.log(line)
  }
  process.exitCode = result.exitCode
}

async function runAddBot(options: AddBotOptions): Promise<void> {
  await withClient(options.profile, async (client) => {
    const beforeInput = await fetchChatAuditInput(client, {
      chatId: options.chat,
      targetUserId: options.targetUser,
    })
    const beforePlan = planAdminBootstrap(beforeInput)

    console.log(`run: ${options.run === true}`)
    console.log(`chat_id: ${beforePlan.chatId}`)
    console.log(`target_user_id: ${beforePlan.targetUserId}`)
    console.log(`target_peer: ${options.targetPeer}`)
    console.log(`actor_role: ${beforePlan.actorRole}`)
    console.log(`target_role_before: ${beforePlan.targetRole}`)
    console.log(
      `proposed_actions: ${formatActionList(beforePlan.proposedActions)}`,
    )

    if (options.run !== true) {
      console.log("dry_run: pass --run to add/promote the bot")
      return
    }
    if (!beforePlan.actorCanApply) {
      throw new Error(
        "Refusing to run: session user cannot apply admin changes",
      )
    }

    if (shouldRunAction(beforePlan.proposedActions, "add target user")) {
      console.log("action: add target user")
      await addTargetToChat(client, {
        chatId: options.chat,
        targetPeer: options.targetPeer,
      })

      const afterAddInput = await fetchChatAuditInput(client, {
        chatId: options.chat,
        targetUserId: options.targetUser,
      })
      const afterAddPlan = planAdminBootstrap(afterAddInput)
      console.log(`target_role_after_add: ${afterAddPlan.targetRole}`)
      console.log(`target_is_admin_after_add: ${afterAddPlan.targetIsAdmin}`)
    }

    if (
      shouldRunAction(beforePlan.proposedActions, "promote target user admin")
    ) {
      console.log("action: promote target user admin")
      await promoteTargetAdmin(client, {
        chatId: options.chat,
        targetPeer: options.targetPeer,
        rights: saiBotAdminRights,
      })
    }

    const afterInput = await fetchChatAuditInput(client, {
      chatId: options.chat,
      targetUserId: options.targetUser,
    })
    const afterPlan = planAdminBootstrap(afterInput)

    console.log(`target_role_after: ${afterPlan.targetRole}`)
    console.log(`target_is_admin_after: ${afterPlan.targetIsAdmin}`)
    console.log(
      `remaining_actions: ${formatActionList(afterPlan.proposedActions)}`,
    )
  })
}

/**
 * Reads a pass-backed 2FA password once, before a `--run` makes its first
 * change, so a locked GPG agent stops the command before it promotes anyone
 * instead of between promotion and ownership transfer.
 */
export async function withPasswordRead(env: TelegramEnv): Promise<TelegramEnv> {
  if (env.password?.kind !== "pass") return env
  return {
    ...env,
    password: { kind: "value", value: await readTelegramPassword(env.password) },
  }
}

async function runAddOwner(options: AddOwnerOptions): Promise<void> {
  await withClient(options.profile, async (client, sessionEnv) => {
    let env = sessionEnv
    const beforeInput = await fetchChatAuditInput(client, {
      chatId: options.chat,
      targetUserId: options.targetUser,
    })
    const beforePlan = planAdminBootstrap(beforeInput)

    console.log(`run: ${options.run === true}`)
    console.log(`elevated: ${env.password !== null}`)
    console.log(`chat_id: ${beforePlan.chatId}`)
    console.log(`target_user: ${options.targetUser}`)
    console.log(`actor_role: ${beforePlan.actorRole}`)
    console.log(`target_role_before: ${beforePlan.targetRole}`)
    console.log(
      `target_can_receive_ownership_before: ${beforePlan.targetCanReceiveOwnership}`,
    )
    console.log(
      `proposed_actions: ${formatActionList(beforePlan.proposedActions)}`,
    )

    if (options.run !== true) {
      console.log("dry_run: pass --run to add/promote the target user")
      return
    }
    if (!beforePlan.actorCanApply) {
      throw new Error(
        "Refusing to run: session user cannot apply admin changes",
      )
    }
    if (beforePlan.actorCanTransferOwnership && env.password !== null) {
      env = await withPasswordRead(env)
    }

    if (shouldRunAction(beforePlan.proposedActions, "add target user")) {
      console.log("action: add target user")
      await addTargetToChat(client, {
        chatId: options.chat,
        targetPeer: options.targetUser,
      })

      const afterAddInput = await fetchChatAuditInput(client, {
        chatId: options.chat,
        targetUserId: options.targetUser,
      })
      const afterAddPlan = planAdminBootstrap(afterAddInput)
      console.log(`target_role_after_add: ${afterAddPlan.targetRole}`)
      console.log(`target_is_admin_after_add: ${afterAddPlan.targetIsAdmin}`)
    }

    if (
      shouldRunAction(beforePlan.proposedActions, "promote target user admin")
    ) {
      console.log("action: promote target user admin")
      await promoteTargetAdmin(client, {
        chatId: options.chat,
        targetPeer: options.targetUser,
        rights: ownerTargetAdminRights,
      })
    }

    const afterInput = await fetchChatAuditInput(client, {
      chatId: options.chat,
      targetUserId: options.targetUser,
    })
    const afterPlan = planAdminBootstrap(afterInput)

    console.log(`target_role_after: ${afterPlan.targetRole}`)
    console.log(`target_is_admin_after: ${afterPlan.targetIsAdmin}`)
    console.log(
      `target_can_receive_ownership_after: ${afterPlan.targetCanReceiveOwnership}`,
    )
    console.log(
      `remaining_actions: ${formatActionList(afterPlan.proposedActions)}`,
    )

    if (!afterPlan.actorCanTransferOwnership) {
      console.log("ownership_transfer: skipped_actor_not_owner")
      return
    }
    if (!afterPlan.targetCanReceiveOwnership) {
      console.log("ownership_transfer: skipped_target_not_eligible")
      return
    }
    if (env.password === null) {
      console.log("ownership_transfer: skipped_missing_telegram_password")
      return
    }

    console.log("ownership_transfer: run")
    await transferTargetOwnership(client, {
      chatId: options.chat,
      targetPeer: options.targetUser,
      password: await readTelegramPassword(env.password),
    })

    const transferInput = await fetchChatAuditInput(client, {
      chatId: options.chat,
      targetUserId: options.targetUser,
    })
    const transferPlan = planAdminBootstrap(transferInput)
    console.log(`target_role_after_transfer: ${transferPlan.targetRole}`)
    console.log(`actor_role_after_transfer: ${transferPlan.actorRole}`)
  })
}

/**
 * Formats the resolved identity of a chat member for command output.
 */
export function formatMemberIdentity(
  member: ChatMemberData | null,
): Record<string, string> {
  if (member === null) {
    return { target_resolved: "none (not in chat)" }
  }
  return {
    target_id: String(member.user.id),
    target_username:
      member.user.username === null ? "none" : `@${member.user.username}`,
    target_display_name: JSON.stringify(member.user.displayName),
    target_is_bot: String(member.user.isBot),
  }
}

/**
 * Runs command `tg-sai promote-admin`: promotes an existing chat member to
 * admin with the owner-target rights. It never adds the target and never
 * transfers ownership, even when the profile has a 2FA password. It refuses
 * unless the session user owns the chat, and it refuses bots and, without
 * `--allow-non-team`, users outside `promoteAdminTeamIds`.
 */
async function runPromoteAdmin(options: PromoteAdminOptions): Promise<void> {
  const planOptions = {
    allowedUserIds: options.allowNonTeam === true ? null : promoteAdminTeamIds,
  }
  await withClient(options.profile, async (client) => {
    const chatTitle = await fetchChatTitle(client, options.chat)
    const beforeInput = await fetchChatAuditInput(client, {
      chatId: options.chat,
      targetUserId: options.targetUser,
    })
    const beforePlan = planPromoteOnly(beforeInput, planOptions)

    console.log(`run: ${options.run === true}`)
    console.log("ownership_transfer: none (promote-only)")
    console.log(`chat_id: ${beforePlan.chatId}`)
    console.log(`chat_title: ${JSON.stringify(chatTitle)}`)
    console.log(`target_user: ${options.targetUser}`)
    for (const [key, value] of Object.entries(
      formatMemberIdentity(beforeInput.target),
    )) {
      console.log(`${key}: ${value}`)
    }
    console.log(`team_allowlist: ${planOptions.allowedUserIds !== null}`)
    console.log(`actor_role: ${beforePlan.actorRole}`)
    console.log(`target_role_before: ${beforePlan.targetRole}`)
    console.log(
      `proposed_actions: ${formatActionList(beforePlan.proposedActions)}`,
    )

    const refusal = beforePlan.proposedActions.find((action) =>
      action.startsWith("refuse: "),
    )
    if (refusal !== undefined) {
      throw new Error(`Refusing to run: ${refusal.slice("refuse: ".length)}`)
    }
    if (!beforePlan.canPromote) {
      return
    }
    if (options.run !== true) {
      console.log("dry_run: pass --run to promote the existing member")
      return
    }

    console.log("action: promote target user admin")
    await promoteExistingMember(client, {
      chatId: options.chat,
      targetPeer: options.targetUser,
      rights: ownerTargetAdminRights,
      expectedUserId: beforeInput.target?.user.id,
    })

    const afterPlan = planPromoteOnly(
      await fetchChatAuditInput(client, {
        chatId: options.chat,
        targetUserId: options.targetUser,
      }),
      planOptions,
    )
    console.log(`target_role_after: ${afterPlan.targetRole}`)
    console.log(`target_is_admin_after: ${afterPlan.targetIsAdmin}`)
  })
}

/**
 * Parses flag `--limit` of command `tg-sai ops sai-find`: a positive integer
 * written as plain digits. Throws on anything else so a typo fails before the
 * command connects to Telegram.
 */
export function parseDialogLimit(value: string): number {
  const limit = /^\d+$/.test(value) ? Number(value) : Number.NaN
  if (!Number.isSafeInteger(limit) || limit < 1) {
    throw new Error(`--limit must be a positive integer, got "${value}"`)
  }
  return limit
}

async function runSaiFind(options: SaiFindOptions): Promise<void> {
  const limit = parseDialogLimit(options.limit)
  await withClient(options.profile, async (client) => {
    const records = await findSaiChats(client, {
      limit,
      archived: options.archived ?? "exclude",
    })
    const lines = records.map(formatSaiChatJsonLine)
    const contents = `${lines.join("\n")}\n`
    const chatFilePath = await nextChatFilePath("sai-find")

    await writeFile(chatFilePath, contents, {
      encoding: "utf8",
      mode: 0o600,
    })
    console.log(`wrote_chat_file: ${chatFilePath}`)
    if (options.out) {
      await writeFile(options.out, contents, {
        encoding: "utf8",
        mode: 0o600,
      })
      console.log(`wrote_out_copy: ${options.out}`)
    }
    console.log(`chat_count: ${records.length}`)
  })
}

/**
 * Plans and optionally applies add/promote actions for one target peer.
 *
 * The same helper handles the Sai bot and the owner-target user. It treats
 * insufficient actor authority as a skip, not a fatal batch error.
 */
async function applyTargetPlan(params: {
  client: Awaited<ReturnType<typeof createMtcuteClient>>
  chat: ChatInput
  actorUsername: string
  targetUserId: number | string
  targetPeer: number | string
  rights: Record<string, boolean>
  actionPrefix: string
  run: boolean
}): Promise<void> {
  const beforeInput = await fetchChatAuditInput(params.client, {
    chatId: params.chat.chatId,
    targetUserId: params.targetUserId,
  })
  const beforePlan = planAdminBootstrap(beforeInput)

  printJsonEvent("target_plan", {
    chatId: params.chat.chatId,
    title: params.chat.title,
    actorUsername: params.actorUsername,
    targetUserId: params.targetUserId,
    actionPrefix: params.actionPrefix,
    run: params.run,
    actorRole: beforePlan.actorRole,
    targetRole: beforePlan.targetRole,
    proposedActions: beforePlan.proposedActions,
  })

  if (!beforePlan.actorCanApply) {
    printJsonEvent("target_skip", {
      chatId: params.chat.chatId,
      actorUsername: params.actorUsername,
      actionPrefix: params.actionPrefix,
      reason: "actor_lacks_owner_or_add_admins_right",
    })
    return
  }

  if (!params.run) {
    return
  }

  const deps = actionExecutorDeps(actorBatchLogger(params.actorUsername))
  if (shouldRunAction(beforePlan.proposedActions, "add target user")) {
    await runTelegramAction(
      {
        chatId: params.chat.chatId,
        action: `${params.actionPrefix}_add`,
      },
      () =>
        addTargetToChat(params.client, {
          chatId: params.chat.chatId,
          targetPeer: params.targetPeer,
        }),
      deps,
    )
  }

  if (
    shouldRunAction(beforePlan.proposedActions, "promote target user admin")
  ) {
    await runTelegramAction(
      {
        chatId: params.chat.chatId,
        action: `${params.actionPrefix}_promote_admin`,
      },
      () =>
        promoteTargetAdmin(params.client, {
          chatId: params.chat.chatId,
          targetPeer: params.targetPeer,
          rights: params.rights,
        }),
      deps,
    )
  }
}

/**
 * Optionally transfers ownership after the target user is already admin.
 *
 * Ownership transfer only runs when the command flag is present, the actor is
 * the current owner, the target user is eligible, and the session has a 2FA
 * password source. A pass entry is read only here, right before the transfer.
 */
async function maybeTransferOwner(params: {
  client: Awaited<ReturnType<typeof createMtcuteClient>>
  env: TelegramEnv
  chat: ChatInput
  actorUsername: string
  targetUser: number | string
  enabled: boolean
  run: boolean
}): Promise<void> {
  if (!params.enabled) {
    return
  }

  const input = await fetchChatAuditInput(params.client, {
    chatId: params.chat.chatId,
    targetUserId: params.targetUser,
  })
  const plan = planAdminBootstrap(input)

  if (!plan.actorCanTransferOwnership) {
    printJsonEvent("ownership_skip", {
      chatId: params.chat.chatId,
      actorUsername: params.actorUsername,
      reason: "actor_not_owner",
    })
    return
  }
  if (!plan.targetCanReceiveOwnership) {
    printJsonEvent("ownership_skip", {
      chatId: params.chat.chatId,
      actorUsername: params.actorUsername,
      reason: "target_not_eligible",
    })
    return
  }
  if (params.env.password === null) {
    printJsonEvent("ownership_skip", {
      chatId: params.chat.chatId,
      actorUsername: params.actorUsername,
      reason: "missing_telegram_password",
    })
    return
  }
  if (!params.run) {
    printJsonEvent("ownership_plan", {
      chatId: params.chat.chatId,
      actorUsername: params.actorUsername,
      action: "transfer_owner",
    })
    return
  }

  await runTelegramAction(
    {
      chatId: params.chat.chatId,
      action: "transfer_owner",
    },
    async () =>
      transferTargetOwnership(params.client, {
        chatId: params.chat.chatId,
        targetPeer: params.targetUser,
        password: await readTelegramPassword(params.env.password!),
      }),
    actionExecutorDeps(actorBatchLogger(params.actorUsername)),
  )
}

/**
 * Runs command `tg-sai ops sai-bootstrap` over a reviewed chat file.
 *
 * The command is dry-run by default. In execution mode, it applies the Sai bot
 * update and owner-target update for each chat, logs per-chat failures, and
 * continues to the next chat. A `PEER_FLOOD` or a flood wait longer than the
 * run's threshold stops the batch with a `flood_stop` event and exit code 1.
 * A refusal by the local flood guard (a saved hold, unreadable hold state,
 * or an unverified session account) stops it the same way with exit 75.
 * `SLOWMODE_WAIT_X` only fails that chat.
 */
async function runSaiBootstrap(options: SaiBootstrapOptions): Promise<void> {
  const contents = await readFile(options.file, "utf8")
  const chats = parseChatInputFile(contents)
  const delayMs = Number(options.delayMs)
  if (!Number.isFinite(delayMs) || delayMs < 0) {
    throw new Error("--delay-ms must be a non-negative number")
  }
  // Telegram calls are in the telemetry DB under this run id.
  const runId = activeTelemetryRun()?.context.runId ?? null

  await withClient(options.profile, async (client, sessionEnv) => {
    let env = sessionEnv
    const actor = normalizeUser(await client.getMe())
    const actorUsername = actor.username ?? ""
    // Only a session from a saved profile is labeled with that profile.
    const profileName =
      env.profileName === null
        ? null
        : await selectedProfileName(options.profile)
    const ownerTarget = resolveBootstrapTarget(options.targetUser)
    const ownershipTransferEnabled =
      options.transferOwner !== false && env.password !== null
    if (runId) console.log(`telemetry_run_id: ${runId}`)
    printJsonEvent("run_start", {
      file: options.file,
      runId,
      profileName,
      actor,
      chatCount: chats.length,
      run: options.run === true,
      transferOwner: ownershipTransferEnabled,
      passwordAvailable: env.password !== null,
      delayMs,
      targetUser: ownerTarget,
      targetBot: saiBot.id,
    })

    if (options.run === true && ownershipTransferEnabled) {
      env = await withPasswordRead(env)
    }

    const maxWaitMs =
      activeTelemetryRun()?.floodMaxWaitMs ??
      floodMaxWaitMs("cli", process.env, BOOTSTRAP_FLOOD_MAX_WAIT_MS)
    const { floodStop } = await runChatBatch(chats, {
      maxWaitMs,
      processItem: async (chat, i) => {
        printJsonEvent("chat_start", {
          index: i + 1,
          total: chats.length,
          chatId: chat.chatId,
          title: chat.title,
          actorUsername,
        })
        await applyTargetPlan({
          client,
          chat,
          actorUsername,
          targetUserId: saiBot.id,
          targetPeer: saiBot.handle,
          rights: saiBotAdminRights,
          actionPrefix: "bot",
          run: options.run === true,
        })
        await applyTargetPlan({
          client,
          chat,
          actorUsername,
          targetUserId: ownerTarget,
          targetPeer: ownerTarget,
          rights: ownerTargetAdminRights,
          actionPrefix: "owner_target",
          run: options.run === true,
        })
        await maybeTransferOwner({
          client,
          env,
          chat,
          actorUsername,
          targetUser: ownerTarget,
          enabled: ownershipTransferEnabled,
          run: options.run === true,
        })
        printJsonEvent("chat_complete", {
          chatId: chat.chatId,
          actorUsername,
        })
      },
      onItemError: (chat, error) => {
        printJsonEvent("chat_error", {
          chatId: chat.chatId,
          actorUsername,
          errorName: error instanceof Error ? error.name : "UnknownError",
          errorMessage:
            error instanceof Error ? error.message : String(error),
        })
      },
      betweenItems: async (chat) => {
        if (delayMs <= 0) return
        printJsonEvent("chat_delay", {
          chatId: chat.chatId,
          actorUsername,
          delayMs,
        })
        await sleep(delayMs)
      },
    })

    if (floodStop) {
      // The rest of the batch would hit the same account-wide limit.
      printJsonEvent("flood_stop", {
        chatId: floodStop.item.chatId,
        actorUsername,
        index: floodStop.index + 1,
        total: chats.length,
        remaining: floodStop.remaining,
        code: floodStop.code,
        waitS: floodStop.waitS,
        resumeAt: floodStop.resumeAt,
        hold: floodStop.hold,
      })
      // A saved hold is a retry-later stop (75); Telegram's own long wait
      // or PEER_FLOOD keeps exit 1.
      process.exitCode = floodStop.hold ? HOLD_EXIT_CODE : 1
      return
    }

    printJsonEvent("run_complete", {
      file: options.file,
      runId,
      chatCount: chats.length,
    })
  })
}

function credentialValuesFromProfileAddOptions(
  options: ProfileAddOptions,
): TelegramCredentialValues {
  return {
    TELEGRAM_API_ID: options.apiId,
    TELEGRAM_API_HASH: options.apiHash,
    TELEGRAM_SESSION_STRING: options.sessionString,
  }
}

function credentialValuesFromEnv(
  env: NodeJS.ProcessEnv = process.env,
): TelegramCredentialValues {
  return {
    TELEGRAM_API_ID: env.TELEGRAM_API_ID,
    TELEGRAM_API_HASH: env.TELEGRAM_API_HASH,
    TELEGRAM_SESSION_STRING: env.TELEGRAM_SESSION_STRING,
    TELEGRAM_PASSWORD: env.TELEGRAM_PASSWORD,
  }
}

function profileFieldsFromCredentialValues(values: TelegramCredentialValues) {
  return {
    apiId: values.TELEGRAM_API_ID ? Number(values.TELEGRAM_API_ID) : null,
    apiHash: values.TELEGRAM_API_HASH ?? "",
    sessionString: values.TELEGRAM_SESSION_STRING ?? "",
    // Profiles keep only a pass entry, so a password from env or discovery
    // is not saved. Use --password-prompt or --password-stdin.
    password: "",
  }
}

type ProfileCredentialFields = ReturnType<
  typeof profileFieldsFromCredentialValues
>

function mergeCredentialValues(
  ...valuesList: Array<TelegramCredentialValues | null>
): TelegramCredentialValues {
  const merged: TelegramCredentialValues = {}
  for (const values of valuesList) {
    if (!values) {
      continue
    }
    for (const [key, value] of Object.entries(values)) {
      if (value !== undefined && value !== "") {
        merged[key as keyof TelegramCredentialValues] = value
      }
    }
  }
  return merged
}

function redactedCredentialValue(key: string, value: string): string {
  if (
    key === "TELEGRAM_API_HASH" ||
    key === "TELEGRAM_SESSION_STRING" ||
    key === "TELEGRAM_PASSWORD"
  ) {
    return value ? "[redacted]" : ""
  }
  return value
}

function sourceLabel(candidate: DiscoveryReport["candidates"][number]): string {
  return `${candidate.name} ${candidate.path} ${candidate.objectPath}`
}

function mergeDiscoveredValues(
  candidates: DiscoveryReport["candidates"],
): TelegramCredentialValues {
  const merged: TelegramCredentialValues = {}
  const origins: Partial<
    Record<
      keyof TelegramCredentialValues,
      DiscoveryReport["candidates"][number]
    >
  > = {}

  for (const candidate of candidates) {
    for (const [key, value] of Object.entries(candidate.values)) {
      if (value === undefined || value === "") {
        continue
      }
      const typedKey = key as keyof TelegramCredentialValues
      const previous = merged[typedKey]
      if (previous !== undefined && previous !== value) {
        const previousOrigin = origins[typedKey]!
        throw new Error(
          [
            `Conflicting Telegram credential values for ${key}.`,
            `- ${sourceLabel(previousOrigin)}: ${redactedCredentialValue(
              key,
              previous,
            )}`,
            `- ${sourceLabel(candidate)}: ${redactedCredentialValue(
              key,
              value,
            )}`,
            "Run `tg-sai profile find` and choose one source with `--src <number>`.",
          ].join("\n"),
        )
      }
      merged[typedKey] = value
      origins[typedKey] = candidate
    }
  }

  return merged
}

function assertNoExplicitSourceConflicts(params: {
  sourceValues: TelegramCredentialValues
  higherPrecedenceValues: TelegramCredentialValues
  sourceDescription: string
}): void {
  for (const [key, value] of Object.entries(params.sourceValues)) {
    if (value === undefined || value === "") {
      continue
    }
    const higherValue =
      params.higherPrecedenceValues[key as keyof TelegramCredentialValues]
    if (
      higherValue !== undefined &&
      higherValue !== "" &&
      higherValue !== value
    ) {
      throw new Error(
        [
          `Conflicting Telegram credential values for ${key}.`,
          `- ${params.sourceDescription}: ${redactedCredentialValue(
            key,
            value,
          )}`,
          `- CLI/env: ${redactedCredentialValue(key, higherValue)}`,
          "Remove the conflicting flag/env var or choose a different `--src`.",
        ].join("\n"),
      )
    }
  }
}

function validateLoginOptions(options: ProfileAddOptions): void {
  if (options.qr && options.phone) {
    throw new Error("--qr and --phone are mutually exclusive")
  }
  if ((options.login || options.qr || options.phone) && options.sessionString) {
    throw new Error("Fresh login cannot be combined with --session-string")
  }
}

/** A new login can reuse app credentials, never another account's session or 2FA. */
function profileLoginValues(
  values: TelegramCredentialValues | null,
  options: ProfileAddOptions,
): TelegramCredentialValues {
  if (!(options.login || options.qr || options.phone)) return values ?? {}
  const { TELEGRAM_SESSION_STRING: _session, TELEGRAM_PASSWORD: _password, ...app } = values ?? {}
  return app
}

async function generateSessionString(params: {
  apiId: number
  apiHash: string
  mode: "qr" | "phone"
}): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "tg-sai-session-"))
  const outputPath = join(dir, "session.txt")
  try {
    const result = Bun.spawnSync({
      cmd: [
        "uv",
        "run",
        "session_string_generator.py",
        `--${params.mode}`,
        "--session-output",
        outputPath,
        "--no-env-write",
      ],
      cwd: import.meta.dir,
      env: {
        ...process.env,
        TELEGRAM_API_ID: String(params.apiId),
        TELEGRAM_API_HASH: params.apiHash,
      },
      stdin: "inherit",
      stdout: "inherit",
      stderr: "inherit",
    })
    if (result.exitCode !== 0) {
      throw new Error(
        `session generator failed with exit code ${result.exitCode}`,
      )
    }
    const session = (await readFile(outputPath, "utf8")).trim()
    if (!session) {
      throw new Error("session generator did not write a session string")
    }
    return session
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
}

async function discoveredValuesForProfileAdd(
  options: ProfileAddOptions,
): Promise<{
  values: TelegramCredentialValues | null
  sourceDescription: string | null
  explicit: boolean
}> {
  if (options.src) {
    const index = Number(options.src)
    if (!Number.isInteger(index) || index < 1) {
      throw new Error("--src must be a 1-based source number")
    }

    const report = await discoveryReport()
    const candidate = report.candidates[index - 1]
    if (!candidate) {
      throw new Error(`Unknown discovery source: ${options.src}`)
    }
    return {
      values: profileLoginValues(candidate.values, options),
      sourceDescription: sourceLabel(candidate),
      explicit: true,
    }
  }

  const report = await discoveryReport()
  return {
    values: mergeDiscoveredValues(report.candidates.map((candidate) => ({
      ...candidate, values: profileLoginValues(candidate.values, options),
    }))),
    sourceDescription: null,
    explicit: false,
  }
}

async function promptForMissingProfileFields(
  profileFields: ProfileCredentialFields,
  options: ProfileAddOptions,
): Promise<void> {
  if (!options.interactive) {
    return
  }

  const rl = createInterface({ input, output })
  try {
    if (!profileFields.apiId) {
      const value = (await rl.question("Telegram API ID: ")).trim()
      if (value) {
        profileFields.apiId = Number(value)
      }
    }
    if (!profileFields.apiHash) {
      const value = (await rl.question("Telegram API hash: ")).trim()
      if (value) {
        profileFields.apiHash = value
      }
    }
    if (
      !profileFields.sessionString &&
      !options.qr &&
      !options.phone &&
      !options.login
    ) {
      const value = (
        await rl.question("Generate a Telegram session now? [Y=QR/n/phone]: ")
      )
        .trim()
        .toLowerCase()
      if (value === "phone" || value === "p") {
        options.phone = true
      } else if (value === "" || value === "y" || value === "yes") {
        options.qr = true
      }
    }
  } finally {
    rl.close()
  }
}

async function inputFromProfileAddOptions(
  options: ProfileAddOptions,
): Promise<ProfileAddInput> {
  validateLoginOptions(options)
  const saved = getProfile(await readConfig(), options.name)
  const enrollsAccount = enrollsNewAccount(options)
  if (saved && !enrollsAccount) {
    // Resolve the saved target before discovery. A password/app metadata edit
    // must never authenticate another discovered account and overwrite this one.
    if (
      (options.userId !== undefined && Number(options.userId) !== saved.userId) ||
      (options.handle !== undefined && options.handle !== saved.handle)
    ) {
      throw new Error("Changing a saved account identity requires an explicit fresh login or session import")
    }
    return {
      name: saved.name,
      apiId: options.apiId === undefined ? saved.apiId : Number(options.apiId),
      apiHash: options.apiHash ?? saved.apiHash,
      sessionString: saved.sessionString,
      password: saved.password,
      handle: saved.handle,
      userId: saved.userId,
      displayName: options.displayName ?? saved.displayName,
    }
  }
  if (!saved && setsPassword(options) && !enrollsAccount) {
    throw unknownPasswordTargetError(options)
  }
  const optionValues = credentialValuesFromProfileAddOptions(options)
  const envValues = profileLoginValues(credentialValuesFromEnv(), options)
  const discovery = await discoveredValuesForProfileAdd(options)
  const higherPrecedenceValues = mergeCredentialValues(envValues, optionValues)
  if (discovery.explicit && discovery.values && discovery.sourceDescription) {
    assertNoExplicitSourceConflicts({
      sourceValues: discovery.values,
      higherPrecedenceValues,
      sourceDescription: discovery.sourceDescription,
    })
  }
  const profileFields = profileFieldsFromCredentialValues(
    mergeCredentialValues(discovery.values, envValues, optionValues),
  )
  await promptForMissingProfileFields(profileFields, options)

  if (
    (options.login || options.qr || options.phone) &&
    !profileFields.sessionString
  ) {
    if (!profileFields.apiId || !profileFields.apiHash) {
      throw new Error(
        [
          "Session generation requires apiId and apiHash. Provide them with one of:",
          "- flags --api-id and --api-hash",
          "- environment variables TELEGRAM_API_ID and TELEGRAM_API_HASH",
          "- file ~/.tg-sai/.env (or $TG_SAI_DIR/.env)",
          "- a source from `tg-sai profile find`, selected with --src <number>",
          "- prompts, with --interactive",
        ].join("\n"),
      )
    }
    profileFields.sessionString = await generateSessionString({
      apiId: profileFields.apiId,
      apiHash: profileFields.apiHash,
      mode: options.phone ? "phone" : "qr",
    })
  }

  if (options.userId && options.displayName) {
    return {
      name: options.name,
      ...profileFields,
      handle: options.handle ?? null,
      userId: Number(options.userId),
      displayName: options.displayName,
    }
  }

  if (
    !profileFields.apiId ||
    !profileFields.apiHash ||
    !profileFields.sessionString
  ) {
    return {
      name: options.name,
      ...profileFields,
      handle: options.handle ?? null,
      userId: null,
      displayName: "",
    }
  }

  const knownUserId =
    options.userId && /^\d+$/.test(options.userId)
      ? Number(options.userId)
      : undefined
  const completeProfileFields: TelegramCredentials = {
    apiId: profileFields.apiId,
    apiHash: profileFields.apiHash,
    sessionString: profileFields.sessionString,
  }
  const client = await createMtcuteClient(
    {
      apiId: completeProfileFields.apiId,
      apiHash: completeProfileFields.apiHash,
      session: completeProfileFields.sessionString,
      // --user-id is unverified, so the guard keys by the session's getMe.
    },
    cliTelemetry({ accountId: knownUserId ?? null, profile: options.name ?? null }),
  )
  try {
    const self = await client.getMe()
    return profileInputFromUser(
      options.name,
      completeProfileFields,
      {
        id: self.id,
        username: self.username ?? null,
        displayName: self.displayName,
        isBot: self.isBot,
        rawType: self.raw?._ ?? "unknown",
      },
      profileFields.password,
    )
  } finally {
    await client.disconnect()
  }
}

async function runProfileList(): Promise<void> {
  const config = await readConfig()
  if (config.profiles.length === 0) {
    console.log("No profiles configured.")
    return
  }
  for (const [index, profile] of config.profiles.entries()) {
    const active = index === 0 ? "*" : " "
    console.log(
      `${active} ${index}\t${profileLabel(profile, index)}\t${profile.userId}\t${profile.displayName}`,
    )
  }
}

async function runProfileShow(selector = "0"): Promise<void> {
  const config = await readConfig()
  const profile = getProfile(config, selector)
  if (!profile) {
    throw new Error(`Unknown profile: ${selector}`)
  }
  console.log(JSON.stringify(redactedProfile(profile), null, 2))
  if (profilePasswordStorage(profile) === "plain_text") {
    console.error(plainTextPasswordWarning(profile.name))
  }
}

/** Warning for a profile that still keeps its 2FA password in profiles.json. */
export function plainTextPasswordWarning(profileName: string): string {
  return `warning: profile ${profileName} keeps its 2FA password in profiles.json; move it to pass with: tg-sai profile migrate-password --profile ${profileName}`
}

/** True when `profile add` options log in or import a session. */
function enrollsNewAccount(options: ProfileAddOptions): boolean {
  return Boolean(
    options.login || options.qr || options.phone || options.src || options.sessionString,
  )
}

function unknownPasswordTargetError(options: ProfileAddOptions): Error {
  return new Error(`Unknown configured profile: ${options.name ?? "active profile"}. Use tg-sai profile list and select it with --name; --src selects credential discovery sources.`)
}

/** True when `profile add` options save or point to a 2FA password. */
function setsPassword(options: ProfileAddOptions): boolean {
  return Boolean(
    options.passwordPassEntry || options.passwordStdin || options.passwordPrompt,
  )
}

/**
 * Reads a new 2FA password from `--password-stdin` or `--password-prompt`,
 * or returns null. Rejects the removed `--password <value>` flag.
 */
async function newPasswordFromOptions(
  options: ProfileAddOptions,
): Promise<string | null> {
  if (options.password !== undefined) {
    throw new Error(
      "--password <value> is no longer supported. Use --password-prompt or --password-stdin; tg-sai stores the password in pass.",
    )
  }
  if (options.passwordStdin && options.passwordPrompt) {
    throw new Error("--password-stdin and --password-prompt are mutually exclusive")
  }
  if (options.passwordStdin) {
    if (options.interactive || options.phone) {
      throw new Error("--password-stdin cannot be combined with --interactive or --phone")
    }
    return readPasswordFromStdin()
  }
  if (options.passwordPrompt) {
    return promptHiddenPassword("Telegram 2FA password: ")
  }
  return null
}

/** pass entry that `profile add` targets, or null when no name is known. */
function profileAddPassEntry(
  options: ProfileAddOptions,
  profileName: string | null,
): string | null {
  const entryName =
    options.passwordPassEntry ??
    (profileName ? defaultPasswordPassEntry(profileName) : null)
  return entryName ? validatePassEntry(entryName) : null
}

/**
 * Checks `entry` before `profile add` writes to it. Returns whether to insert.
 * The profile's own entries (the one it records and its default
 * `tg-sai/<name>/telegram-2fa`) may be replaced. Any other existing entry may
 * hold another secret, so it is refused without being read.
 */
async function checkProfileAddPassEntry(
  entry: string,
  newPassword: string | null,
  ownEntries: Array<string | null | undefined>,
): Promise<boolean> {
  const exists = await passEntryExists(entry)
  if (newPassword === null) {
    if (!exists) {
      throw new Error(
        `pass entry ${entry} does not exist. Add --password-prompt or --password-stdin to create it.`,
      )
    }
    return false
  }
  if (exists && !ownEntries.includes(entry)) {
    throw new Error(
      `pass entry ${entry} already exists and is not this profile's entry. Choose another --password-pass-entry, or drop --password-prompt/--password-stdin to point the profile at the existing entry.`,
    )
  }
  return true
}

/**
 * Entries that belong to profile `name`: the one it records, and its default
 * unless another profile records that default.
 */
function ownPassEntries(
  name: string | null,
  recorded: string | undefined,
  otherProfilesEntries: readonly string[],
): Array<string | undefined> {
  const fallback = name ? defaultPasswordPassEntry(name) : undefined
  return [
    recorded,
    fallback && !otherProfilesEntries.includes(fallback) ? fallback : undefined,
  ]
}

/**
 * Saves `newPassword` in pass, or checks that `--password-pass-entry`
 * exists, and returns the entry to record on the profile.
 */
async function passwordPassEntryForProfileAdd(
  options: ProfileAddOptions,
  newPassword: string | null,
  profileName: string | null,
  ownEntry: string | undefined,
  otherProfilesEntries: readonly string[],
): Promise<string | null> {
  if (newPassword === null && !options.passwordPassEntry) return null
  const entry = profileAddPassEntry(options, profileName)
  if (!entry) {
    throw new Error("Name the profile with --name or the pass entry with --password-pass-entry")
  }
  if (
    await checkProfileAddPassEntry(
      entry,
      newPassword,
      ownPassEntries(profileName, ownEntry, otherProfilesEntries),
    )
  ) {
    await insertPassPassword(entry, newPassword!)
  }
  return entry
}

async function printConfiguredProfiles(): Promise<void> {
  console.log("\nConfigured local profiles (* marks the active/default profile):")
  await runProfileList()
}

async function printProfileAddHelp(command: Command): Promise<void> {
  command.outputHelp()
  console.log(
    "\nNo profile is saved and no Telegram login starts without explicit options.",
  )
  console.log(
    "Use a new --name to add a separate account. Without --name, explicit options update the active profile.",
  )
  console.log("\nFresh QR login for a separate custody account:")
  console.log(
    "  tg-sai profile add --name sai-custody --login --qr --interactive",
  )
  console.log(
    "Scan while signed into the intended Telegram account, then inspect it with:",
  )
  console.log("  tg-sai profile show sai-custody")
  await printConfiguredProfiles()
  console.log("\nInspect an existing profile with tg-sai profile show <selector>.")
  console.log("Save a 2FA password in pass with --name <saved-name-or-handle-or-index> --password-prompt.")
  console.log("--src selects a numbered credential source from profile find, not a saved profile index.")
}

async function printProfileRemoveHelp(command: Command): Promise<void> {
  command.outputHelp()
  console.log(
    "\nRemoves a saved local profile, not the Telegram account or its chat memberships.",
  )
  console.log("Removing a profile does not revoke its Telegram sessions.")
  console.log("No profile is removed without an explicit selector.")
  const config = await readConfig()
  if (config.profiles.length === 0) {
    console.log("\nNo saved profiles are available to remove.")
    console.log(
      "Add an account with: tg-sai profile add --login --qr --interactive",
    )
    return
  }

  console.log("\nRunnable examples for saved profiles:")
  let examples = 0
  for (const [index, saved] of config.profiles.entries()) {
    // Handles and names can overlap. Emit only selectors that resolve to this row.
    const selector = [
      saved.handle,
      saved.name,
      saved.userId?.toString(),
      String(index),
    ].find(
      (candidate) =>
        candidate != null && findProfileIndex(config, candidate) === index,
    )
    if (selector == null) continue
    const argument = /^[A-Za-z0-9_@./:-]+$/.test(selector)
      ? selector
      : `'${selector.replaceAll("'", "'\\''")}'`
    const label = JSON.stringify(profileLabel(saved, index))
    console.log(
      `  tg-sai profile remove -- ${argument}  # ${label}${index === 0 ? " [active]" : ""}`,
    )
    examples++
  }
  if (examples < config.profiles.length) {
    console.log(
      "Some profiles have ambiguous selectors; inspect tg-sai profile list before removing them.",
    )
  }
  console.log(
    "\nExamples use the current profile configuration. Run tg-sai profile list to inspect it.",
  )
}

async function runProfileAdd(options: ProfileAddOptions): Promise<void> {
  const savedConfig = await readConfig()
  const previous = getProfile(savedConfig, options.name)
  // Pass entries other profiles record; never treated as this profile's own.
  const otherProfilesEntries = savedConfig.profiles
    .filter((profile) => profile !== previous)
    .flatMap((profile) => (profile.passwordPassEntry ? [profile.passwordPassEntry] : []))
  // Fail before asking for a password that has no profile to go to.
  if (!previous && setsPassword(options) && !enrollsNewAccount(options)) {
    throw unknownPasswordTargetError(options)
  }
  // A login names the profile only after it creates a Telegram session, too
  // late to check the pass entry without discarding that session.
  if (
    setsPassword(options) &&
    (options.login || options.qr || options.phone) &&
    !options.name &&
    !options.passwordPassEntry
  ) {
    throw new Error(
      "Saving a password with a login needs --name or --password-pass-entry, so the pass entry is checked before the login starts",
    )
  }
  // Read stdin before login or prompts can consume it.
  const newPassword = await newPasswordFromOptions(options)
  if (newPassword !== null || options.passwordPassEntry) {
    // Check pass before a login creates a session that a failed insert
    // would leave unsaved.
    await assertPassReady()
    const earlyName = previous?.name ?? options.name ?? null
    const earlyEntry = profileAddPassEntry(options, earlyName)
    if (earlyEntry) {
      await checkProfileAddPassEntry(
        earlyEntry,
        newPassword,
        ownPassEntries(earlyName, previous?.passwordPassEntry, otherProfilesEntries),
      )
    }
  }
  const input = await inputFromProfileAddOptions(options)
  if (previous?.userId != null && input.userId !== previous.userId) {
    throw new Error("Login or import does not match the saved account. Use a new --name for another account, or explicitly remove the old profile before replacing it.")
  }
  if (previous) input.name = previous.name
  const passwordPassEntry = await passwordPassEntryForProfileAdd(
    options,
    newPassword,
    input.name ??
      input.handle ??
      (input.userId !== null ? String(input.userId) : null),
    previous?.passwordPassEntry,
    otherProfilesEntries,
  )
  if (passwordPassEntry) input.passwordPassEntry = passwordPassEntry
  const config = await addProfile(input)
  const profile = getProfile(config, input.name)
  const index = profile ? config.profiles.indexOf(profile) : 0
  const savedProfile = profile ?? config.profiles[0]!
  const active = index === 0 ? "default" : "saved"
  console.log(`profile_${active}: ${profileLabel(savedProfile, index)}`)
  console.log(`profile_index: ${index}`)
  console.log(`user_id: ${input.userId}`)
  console.log(`handle: ${input.handle ?? ""}`)
  const storage = profilePasswordStorage(savedProfile)
  console.log(
    `telegram_password: ${
      storage === "pass"
        ? `stored in pass entry ${savedProfile.passwordPassEntry}`
        : storage === "plain_text"
          ? "stored in profiles.json"
          : "missing"
    }`,
  )
  if (storage === "plain_text") {
    console.error(plainTextPasswordWarning(savedProfile.name))
  }
  console.log(
    "ownership_transfer: a profile password in pass enables transfer",
  )
}

function printDiscoveryReport(report: DiscoveryReport): void {
  console.log(`platform: ${report.platform}`)
  console.log(`home: ${report.home}`)
  console.log("checked:")
  for (const path of report.checkedPaths) {
    const found = report.foundFiles.includes(path.expandedPath)
      ? "found"
      : "missing"
    console.log(`  - ${path.name}: ${path.path} [${found}]`)
  }

  console.log("candidates:")
  if (report.candidates.length === 0) {
    console.log("  - none")
    return
  }

  for (const [index, candidate] of report.candidates.entries()) {
    const status =
      candidate.missingKeys.length === 0
        ? "complete"
        : `missing ${candidate.missingKeys.join(",")}`
    console.log(
      `  ${index + 1}. ${candidate.name}: ${candidate.path} ${candidate.objectPath} [${status}]`,
    )
  }
  console.log("next:")
  try {
    const values = mergeDiscoveredValues(report.candidates)
    if (Object.keys(values).length === 0) {
      console.log("  tg-sai profile add")
      return
    }
    console.log("  tg-sai profile add")
    if (!values.TELEGRAM_SESSION_STRING) {
      console.log("  tg-sai profile add --qr")
      console.log("  tg-sai profile add --phone")
    }
  } catch (error) {
    console.log(
      `  conflict: ${error instanceof Error ? error.message : String(error)}`,
    )
    console.log("  tg-sai profile add --src <number>")
  }
}

export function createProgram(): Command {
  const program = new Command()

  program
    .name("tg-sai")
    .description("Audit and manage Sai Telegram chat admin state")
    .version(packageJson.version, "--version", "output the version number")
    .helpOption("--help", "display help for command")

  program.action(() => {
    program.help({ error: false })
  })

  program.hook("preAction", (_root, actionCommand) => {
    startCliTelemetryRun(actionCommand)
  })

  addRuntimeHealthCommands(program, runHealth)

  program
    .command("audit")
    .description("Read-only audit for one chat and target user")
    .option("--profile <name>", "Profile name to use for Telegram credentials")
    .requiredOption("--chat <id>", "Telegram chat ID or username", parsePeer)
    .option(
      "--target-user <id>",
      "Target user ID or username",
      parsePeer,
      DEFAULT_USER_TARGET_ID,
    )
    .addHelpText(
      "after",
      `
Examples:
  tg-sai audit --chat -1001234567890
  tg-sai audit --chat @example_sai_chat --target-user @example_user
`,
    )
    .action(async (options: BaseCliOptions) => {
      await runAudit(options)
    })

  program
    .command("add-bot")
    .description("Add SaiTeam_bot and promote it to admin when possible")
    .option("--profile <name>", "Profile name to use for Telegram credentials")
    .requiredOption("--chat <id>", "Telegram chat ID or username", parsePeer)
    .option(
      "--target-user <id>",
      "Bot user ID used for membership audit",
      parsePeer,
      DEFAULT_BOT_TARGET_ID,
    )
    .option(
      "--target-peer <peer>",
      "Bot username or peer used for add/promote writes",
      parsePeer,
      DEFAULT_BOT_TARGET_PEER,
    )
    .option("--run", "Execute add/promote actions; default is dry-run")
    .action(async (options: AddBotOptions) => {
      await runAddBot(options)
    })

  program
    .command("add-owner")
    .description("Add a target user as owner if possible, otherwise admin")
    .option("--profile <name>", "Profile name to use for Telegram credentials")
    .requiredOption("--chat <id>", "Telegram chat ID or username", parsePeer)
    .requiredOption(
      "--target-user <peer>",
      "Target user ID or username",
      parsePeer,
    )
    .option("--run", "Execute add/promote actions; default is dry-run")
    .action(async (options: AddOwnerOptions) => {
      await runAddOwner(options)
    })

  program
    .command("promote-admin")
    .description(
      "Promote an existing member to admin without transferring ownership",
    )
    .option("--profile <name>", "Profile name to use for Telegram credentials")
    .requiredOption("--chat <id>", "Telegram chat ID or username", parsePeer)
    .requiredOption(
      "--target-user <peer>",
      "Existing member: user ID or username",
      parsePeer,
    )
    .option("--run", "Execute the promotion; default is dry-run")
    .option(
      "--allow-non-team",
      "Allow a target outside the team accounts in cfg.ts USERS",
    )
    .addHelpText(
      "after",
      `
Refuses when the target is not already a member, is a bot, or is outside the
team accounts (unless --allow-non-team). The session user must own the chat.
Uses the add-owner admin rights, including add-admins, and never transfers
ownership.

Examples:
  tg-sai promote-admin --chat -1001234567890 --target-user @example_user
  tg-sai promote-admin --chat -1001234567890 --target-user @example_user --run
`,
    )
    .action(async (options: PromoteAdminOptions) => {
      await runPromoteAdmin(options)
    })

  const ops = program
    .command("ops")
    .description("Sai Telegram operations for chat discovery and updates")

  ops
    .command("sai-find")
    .description(
      "Find Sai-related chats visible to the active Telegram session",
    )
    .option("--profile <name>", "Profile name to use for Telegram credentials")
    .option("--limit <count>", "Maximum dialogs to inspect", "500")
    .option(
      "--archived <mode>",
      "Archive mode: exclude, include, or only",
      "exclude",
    )
    .option("--out <path>", "Also write a copy of the JSONL chat records")
    .action(async (options: SaiFindOptions) => {
      await runSaiFind(options)
    })

  ops
    .command("sai-bootstrap")
    .description("Apply Sai bot and owner-target updates from a chat file")
    .option("--profile <name>", "Profile name to use for Telegram credentials")
    .requiredOption("--file <path>", "Chat input file")
    .option(
      "--delay-ms <ms>",
      "Delay between chats in milliseconds",
      String(DEFAULT_INTER_CHAT_DELAY_MS),
    )
    .option("--run", "Execute actions; default is dry-run")
    .option(
      "--no-transfer-owner",
      "Skip ownership transfer even when a 2FA password is available",
    )
    .option(
      "--target-user <peer>",
      "Owner target user ID or username; default is the Sai custody account",
      parsePeer,
    )
    .action(async (options: SaiBootstrapOptions) => {
      await runSaiBootstrap(options)
    })

  const profile = program
    .command("profile")
    .description("Manage local Telegram session profiles")
    .action(async () => {
      profile.outputHelp()
      await printConfiguredProfiles()
    })

  profile
    .command("find")
    .description("Find Telegram credential sources without adding a profile")
    .action(async () => {
      printDiscoveryReport(await discoveryReport())
    })

  profile
    .command("list")
    .description("List configured profiles")
    .action(async () => {
      await runProfileList()
    })

  const addProfileCommand = profile
    .command("add")
    .description("Add or update a Telegram session profile")
    .option(
      "--name <name>",
      "Local profile name; use a distinct name for another account",
    )
    .option("--api-id <id>", "Telegram API ID")
    .option("--api-hash <hash>", "Telegram API hash")
    .option("--session-string <value>", "Telegram session string")
    .addOption(new Option("--password <value>").hideHelp())
    .option(
      "--password-prompt",
      "Prompt for the Telegram 2FA password without echo and save it in pass",
    )
    .option(
      "--password-stdin",
      "Read the Telegram 2FA password from stdin and save it in pass",
    )
    .option(
      "--password-pass-entry <entry>",
      "pass entry for the 2FA password (default tg-sai/<name>/telegram-2fa)",
    )
    .option("--src <number>", "Import a numbered credential source from profile find, not a saved profile")
    .option(
      "--login",
      "Start a fresh account login, ignoring discovered sessions (defaults to QR)",
    )
    .option("--qr", "Generate a session string with QR login")
    .option("--phone", "Generate a session string with phone login")
    .option("-i, --interactive", "Prompt for missing profile values")
    .option("--handle <handle>", "Resolved Telegram handle")
    .option("--user-id <id>", "Resolved Telegram user ID")
    .option("--display-name <name>", "Resolved Telegram display name")
    .addHelpText(
      "after",
      `
Examples:
  tg-sai profile add --name sai-custody --login --qr --interactive
  tg-sai profile add --name another-account --phone --interactive
  tg-sai profile add --name another-account --src 1
  tg-sai profile add --name existing-profile --password-prompt
  pass show my/entry | tg-sai profile add --name existing-profile --password-stdin
  tg-sai profile add --name existing-profile --password-pass-entry my/entry

If apiId/apiHash are missing, create a Telegram API app at:
  https://my.telegram.org/apps
`,
    )
    .action(async (options: ProfileAddOptions) => {
      if (Object.keys(options).length === 0) {
        await printProfileAddHelp(addProfileCommand)
        return
      }
      await runProfileAdd(options)
    })

  profile
    .command("migrate-password")
    .description(
      "Move plain-text 2FA passwords from profiles.json and backups into pass",
    )
    .option(
      "--profile <selector>",
      "Profile to migrate; default is every profile with a plain-text password",
    )
    .option(
      "--pass-entry <entry>",
      "pass entry for --profile (default tg-sai/<name>/telegram-2fa)",
    )
    .option("--run", "Apply changes; default is dry-run")
    .action(async (options: MigratePasswordOptions) => {
      const result = await migrateProfilePasswords(options)
      for (const line of result.lines) console.log(line)
      process.exitCode = result.exitCode
    })

  profile
    .command("show")
    .description("Show one configured profile")
    .argument(
      "[selector]",
      "Profile index, handle, user ID, or legacy name",
      "0",
    )
    .action(async (selector: string) => {
      await runProfileShow(selector)
    })

  profile
    .command("use")
    .description("Set the active profile")
    .argument("<selector>", "Profile index, handle, user ID, or legacy name")
    .action(async (selector: string) => {
      const config = await setActiveProfile(selector)
      const active = config.profiles[0]!
      console.log(`active_profile: ${profileLabel(active, 0)}`)
    })

  const removeProfileCommand = profile
    .command("remove")
    .description("Remove one configured profile")
    .argument(
      "[selector]",
      "Profile index, handle, user ID, or legacy name; omit for examples",
    )
    .action(async (selector: string | undefined) => {
      if (selector === undefined) {
        await printProfileRemoveHelp(removeProfileCommand)
        return
      }
      await removeProfile(selector)
      console.log(`removed_profile: ${selector}`)
    })

  addProvisionCommands(program)
  addCustodyWorkerCommands(program)
  registerEventsCommand(program)
  registerHoldsCommand(program)
  installBareHelp(program)
  return program
}

/** Space-joined subcommand path below the root, e.g. `ops sai-find`. */
function commandPath(command: Command): string {
  const names: string[] = []
  for (let c: Command | null = command; c?.parent; c = c.parent) {
    names.unshift(c.name())
  }
  return names.join(" ")
}

/** Telemetry subsystem for a subcommand path. */
function subsystemFor(path: string): TelemetrySubsystem {
  if (path === "worker") return "worker"
  if (path === "health") return "health"
  return "cli"
}

/** Commands that only read telemetry; they do not record runs of their own. */
function skipsTelemetry(path: string): boolean {
  return path === "" || path === "events" || path.startsWith("events ")
}

/** Exit code for a run stopped by SIGINT/SIGTERM (128 + SIGINT). */
const SIGNAL_EXIT_CODE = 130

/**
 * Ends the run as aborted on the first SIGINT/SIGTERM, then exits. The
 * `worker` command installs its own handlers for a graceful stop.
 */
function endRunOnSignal(path: string): void {
  if (path === "worker") return
  const onSignal = () => {
    process.removeListener("SIGINT", onSignal)
    process.removeListener("SIGTERM", onSignal)
    activeTelemetryRun()?.end({
      exitCode: SIGNAL_EXIT_CODE,
      outcome: "aborted",
    })
    process.exit(SIGNAL_EXIT_CODE)
  }
  process.once("SIGINT", onSignal)
  process.once("SIGTERM", onSignal)
}

function startCliTelemetryRun(actionCommand: Command): void {
  const path = commandPath(actionCommand)
  if (helpOnlyCommands.has(actionCommand)) return
  if (skipsTelemetry(path) || activeTelemetryRun()) return
  startTelemetryRun({
    subsystem: subsystemFor(path),
    command: path,
    argv: process.argv.slice(2),
    version: packageJson.version,
    ...(path === "ops sai-bootstrap"
      ? { floodMaxWaitMs: BOOTSTRAP_FLOOD_MAX_WAIT_MS }
      : {}),
  })
  endRunOnSignal(path)
}

// ---------------------------------------------------------------------------
// Bare commands print help

/** Groups whose only action prints help; they record no telemetry run. */
const helpOnlyCommands = new WeakSet<Command>()

/** Commander keeps the action handler private; read it without casting. */
function hasAction(command: Command): boolean {
  return (command as unknown as { _actionHandler?: unknown })._actionHandler
    ? true
    : false
}

/**
 * Makes every command group without an action print its help when called
 * bare, with exit code 0 (commander's default is help on stderr, exit 1).
 */
export function installBareHelp(command: Command): void {
  for (const sub of command.commands) installBareHelp(sub)
  if (command.commands.length === 0 || hasAction(command)) return
  helpOnlyCommands.add(command)
  command.action(() => {
    command.help({ error: false })
  })
}

/** True when the command has a required argument or mandatory option. */
export function hasRequiredInput(command: Command): boolean {
  return (
    command.registeredArguments.some((arg) => arg.required) ||
    command.options.some((option) => option.mandatory)
  )
}

/**
 * The leaf command `args` name when it has required input and `args` hold
 * nothing past its name, e.g. `tg-sai audit`. Such a call prints that
 * command's help instead of a missing-option error. Any extra token, even a
 * wrong one, returns null so commander reports the usual error.
 */
export function bareHelpTarget(
  program: Command,
  args: readonly string[],
): Command | null {
  let command = program
  for (const arg of args) {
    const next = command.commands.find(
      (sub) => sub.name() === arg || sub.aliases().includes(arg),
    )
    if (!next) return null
    command = next
  }
  if (command === program || command.commands.length > 0) return null
  return hasRequiredInput(command) ? command : null
}

// ---------------------------------------------------------------------------
// Exit codes

/** `EX_TEMPFAIL`: retry later. Used while Telegram limits the account. */
export const HOLD_EXIT_CODE = 75
const HOLD_STATE_UNAVAILABLE = "TG_SAI_HOLD_STATE_UNAVAILABLE"

export interface HoldExit {
  exitCode: typeof HOLD_EXIT_CODE
  /** One line for stderr. */
  message: string
}

interface HoldLike {
  code?: unknown
  text?: unknown
  message?: unknown
  untilMs?: unknown
  grp?: unknown
  cause?: unknown
}

function holdText(error: HoldLike): string | null {
  if (typeof error.text === "string") return error.text
  if (typeof error.message === "string") return error.message
  return null
}

function holdLike(value: unknown): HoldLike | null {
  return typeof value === "object" && value !== null
    ? (value as HoldLike)
    : null
}

/** UTC timestamp without milliseconds, e.g. `2026-11-06T16:20:05Z`. */
function utcSeconds(ms: number): string {
  return new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z")
}

/** `4h 16m`, `12m 5s`, or `40s`. */
export function shortDuration(totalS: number): string {
  const s = Math.max(0, Math.round(totalS))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  if (h > 0) return `${h}h ${m}m`
  if (m > 0) return `${m}m ${s % 60}s`
  return `${s}s`
}

/**
 * Maps a saved flood hold (`HoldError`, `TG_SAI_HOLD_<s>`, code 420), an
 * unreadable `state.db` (`TG_SAI_HOLD_STATE_UNAVAILABLE`), or an unverified
 * session account (`TG_SAI_HOLD_ACCOUNT_UNKNOWN`) to exit 75 with a
 * one-line message. Looks through `error.cause` chains. Returns null for
 * every other error.
 */
export function holdExitFor(
  error: unknown,
  nowMs: number = Date.now(),
): HoldExit | null {
  let current = holdLike(error)
  for (let depth = 0; current && depth < 8; depth += 1) {
    const text = holdText(current)
    if (text === HOLD_STATE_UNAVAILABLE) {
      return {
        exitCode: HOLD_EXIT_CODE,
        message:
          "tg-sai: cannot read saved Telegram limits in " +
          `${stateDbPath()}; invite/create calls stay blocked and nothing ` +
          "was sent. Fix the file, then retry.",
      }
    }
    if (text === HOLD_ACCOUNT_UNKNOWN) {
      return {
        exitCode: HOLD_EXIT_CODE,
        message:
          "tg-sai: could not confirm which Telegram account this session " +
          "acts as, so its saved limits cannot be checked; invite/create " +
          "calls stay blocked and nothing was sent. Retry once getMe works.",
      }
    }
    const seconds = holdErrorSeconds(text)
    const codeOk = current.code === undefined || current.code === 420
    if (seconds !== null && codeOk) {
      const untilMs =
        typeof current.untilMs === "number" && Number.isFinite(current.untilMs)
          ? current.untilMs
          : nowMs + seconds * 1000
      const grp = typeof current.grp === "string" ? current.grp : "this"
      const waitS = Math.max(0, Math.ceil((untilMs - nowMs) / 1000))
      return {
        exitCode: HOLD_EXIT_CODE,
        message:
          `tg-sai: Telegram is limiting ${grp} calls for this account; ` +
          `nothing was sent. Resume after ${utcSeconds(untilMs)} ` +
          `(in ${shortDuration(waitS)}).`,
      }
    }
    current = holdLike(current.cause)
  }
  return null
}

// ---------------------------------------------------------------------------
// Entry point

/**
 * Run the tg-sai CLI on process.argv. Exported so the compiled custody worker
 * (sai-deal-desk deploy/botService.ts) runs the same program, including
 * telemetry runs, hold exit codes, and bare-command help.
 */
export async function runCli(): Promise<void> {
  const program = createProgram()
  const args = process.argv.slice(2)
  const helpTarget = bareHelpTarget(program, args)
  if (helpTarget) {
    helpTarget.outputHelp()
    return
  }
  try {
    await program.parseAsync()
  } catch (error) {
    const hold = holdExitFor(error)
    const exitCode = hold?.exitCode ?? 1
    activeTelemetryRun()?.end({ exitCode, error })
    if (hold) console.error(hold.message)
    else console.error(error)
    process.exit(exitCode)
  }
  activeTelemetryRun()?.end({ exitCode: Number(process.exitCode ?? 0) })
}

if (import.meta.main) await runCli()
