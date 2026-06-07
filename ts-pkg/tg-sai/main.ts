#!/usr/bin/env bun
import { appendFileSync } from "node:fs"
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
import { Command } from "commander"

import type { BatchLogger } from "./actionExecutor"
import { withFloodWaitRetry } from "./actionExecutor"
import type {
  ChatAuditInput,
  ChatMemberData,
  ProposedAction,
} from "./adminAudit"
import { planAdminBootstrap } from "./adminAudit"
import { saiBot, uniqueDivine } from "./cfg"
import { runHealthCheck } from "./health"
import {
  addTargetToChat,
  createMtcuteClient,
  fetchChatAuditInput,
  findSaiChats,
  promoteTargetAdmin,
  transferTargetOwnership,
} from "./mtcuteAdapter"
import { formatSaiChatJsonLine, parseChatInputFile } from "./ops"
import type { ChatInput } from "./ops"
import {
  addProfile,
  configDir,
  discoveryReport,
  getProfile,
  profileInputFromUser,
  readConfig,
  redactedProfile,
  removeProfile,
  resolveTelegramCredentials,
  setActiveProfile,
} from "./profiles"
import type {
  DiscoveryReport,
  ProfileAddInput,
  TelegramCredentials,
  TelegramCredentialValues,
} from "./profiles"

const DEFAULT_USER_TARGET_ID = uniqueDivine.id
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
}

interface TelegramEnv {
  apiId: number
  apiHash: string
  session: string
  password: string | null
}

interface ProfileAddOptions {
  name: string
  apiId?: string
  apiHash?: string
  sessionString?: string
  password?: string
  src?: string
  handle?: string
  userId?: string
  displayName?: string
  login?: boolean
  qr?: boolean
  phone?: boolean
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
let activeJsonLogPath: string | null = null

/**
 * Emits one structured JSON object to stdout and the active JSONL log, if set.
 */
function emitJsonObject(fields: Record<string, unknown>): void {
  const line = JSON.stringify({ ts: new Date().toISOString(), ...fields })
  console.log(line)
  if (activeJsonLogPath) {
    appendFileSync(activeJsonLogPath, `${line}\n`, "utf8")
  }
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
 * Builds the default retry dependencies for live Telegram actions.
 */
function actionExecutorDeps(logger: BatchLogger = consoleBatchLogger) {
  return {
    logger,
    sleep,
    now: () => Date.now(),
    maxRetries: 3,
    bufferMs: 2000,
  }
}

/**
 * Emits one command-level JSON event to stdout.
 */
function printJsonEvent(type: string, fields: Record<string, unknown>): void {
  emitJsonObject({ type, ...fields })
}

/**
 * Returns the directory for durable tg-sai run and discovery logs.
 */
function logsDir(): string {
  return join(configDir(), "logs")
}

/**
 * Chooses the next numbered JSONL log path for a named command prefix.
 *
 * Example output paths are `sai-find-00.jsonl`, `sai-find-01.jsonl`, and so on.
 */
async function nextJsonlLogPath(prefix: string): Promise<string> {
  const dir = logsDir()
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

async function getTelegramEnv(profileName?: string): Promise<TelegramEnv> {
  const credentials = await resolveTelegramCredentials(profileName)
  const config = await readConfig()
  const profile = getProfile(config, profileName)
  return {
    apiId: credentials.apiId,
    apiHash: credentials.apiHash,
    session: credentials.sessionString,
    password: process.env.TELEGRAM_PASSWORD ?? profile?.password ?? null,
  }
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
  console.log(`proposed_actions: ${audit.proposedActions.join("; ")}`)
}

function shouldRunAction(
  proposedActions: ProposedAction[],
  action: ProposedAction,
): boolean {
  return proposedActions.includes(action)
}

async function withClient<T>(
  profileName: string | undefined,
  fn: (
    client: Awaited<ReturnType<typeof createMtcuteClient>>,
    env: TelegramEnv,
  ) => Promise<T>,
): Promise<T> {
  const env = await getTelegramEnv(profileName)
  const client = await createMtcuteClient({
    apiId: env.apiId,
    apiHash: env.apiHash,
    session: env.session,
  })

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
  const result = await runHealthCheck({
    createClient: async (credentials) =>
      createMtcuteClient({
        apiId: credentials.apiId,
        apiHash: credentials.apiHash,
        session: credentials.sessionString,
      }),
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
    console.log(`proposed_actions: ${beforePlan.proposedActions.join("; ")}`)

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
    console.log(`remaining_actions: ${afterPlan.proposedActions.join("; ")}`)
  })
}

async function runAddOwner(options: AddOwnerOptions): Promise<void> {
  await withClient(options.profile, async (client, env) => {
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
    console.log(`proposed_actions: ${beforePlan.proposedActions.join("; ")}`)

    if (options.run !== true) {
      console.log("dry_run: pass --run to add/promote the target user")
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
    console.log(`remaining_actions: ${afterPlan.proposedActions.join("; ")}`)

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
      password: env.password,
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

async function runSaiFind(options: SaiFindOptions): Promise<void> {
  await withClient(options.profile, async (client) => {
    const records = await findSaiChats(client, {
      limit: Number(options.limit),
      archived: options.archived ?? "exclude",
    })
    const lines = records.map(formatSaiChatJsonLine)
    const contents = `${lines.join("\n")}\n`
    const logPath = await nextJsonlLogPath("sai-find")

    await writeFile(logPath, contents, {
      encoding: "utf8",
      mode: 0o600,
    })
    console.log(`wrote_chat_file: ${logPath}`)
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
      actionPrefix: params.actionPrefix,
      reason: "actor_lacks_owner_or_add_admins_right",
    })
    return
  }

  if (!params.run) {
    return
  }

  const deps = actionExecutorDeps()
  if (shouldRunAction(beforePlan.proposedActions, "add target user")) {
    await withFloodWaitRetry(
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
    await withFloodWaitRetry(
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
 * the current owner, the target user is eligible, and `TELEGRAM_PASSWORD` is
 * present in the environment.
 */
async function maybeTransferOwner(params: {
  client: Awaited<ReturnType<typeof createMtcuteClient>>
  env: TelegramEnv
  chat: ChatInput
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
      reason: "actor_not_owner",
    })
    return
  }
  if (!plan.targetCanReceiveOwnership) {
    printJsonEvent("ownership_skip", {
      chatId: params.chat.chatId,
      reason: "target_not_eligible",
    })
    return
  }
  if (params.env.password === null) {
    printJsonEvent("ownership_skip", {
      chatId: params.chat.chatId,
      reason: "missing_telegram_password",
    })
    return
  }
  if (!params.run) {
    printJsonEvent("ownership_plan", {
      chatId: params.chat.chatId,
      action: "transfer_owner",
    })
    return
  }

  await withFloodWaitRetry(
    {
      chatId: params.chat.chatId,
      action: "transfer_owner",
    },
    () =>
      transferTargetOwnership(params.client, {
        chatId: params.chat.chatId,
        targetPeer: params.targetUser,
        password: params.env.password ?? "",
      }),
    actionExecutorDeps(),
  )
}

/**
 * Runs command `tg-sai ops sai-bootstrap` over a reviewed chat file.
 *
 * The command is dry-run by default. In execution mode, it applies the Sai bot
 * update and owner-target update for each chat, logs per-chat failures, and
 * continues to the next chat.
 */
async function runSaiBootstrap(options: SaiBootstrapOptions): Promise<void> {
  const contents = await readFile(options.file, "utf8")
  const chats = parseChatInputFile(contents)
  const delayMs = Number(options.delayMs)
  if (!Number.isFinite(delayMs) || delayMs < 0) {
    throw new Error("--delay-ms must be a non-negative number")
  }
  const logPath = await nextJsonlLogPath("sai-bootstrap")
  await writeFile(logPath, "", {
    encoding: "utf8",
    mode: 0o600,
  })

  await withClient(options.profile, async (client, env) => {
    activeJsonLogPath = logPath
    const ownershipTransferEnabled =
      options.transferOwner !== false && env.password !== null
    console.log(`wrote_run_log: ${logPath}`)
    printJsonEvent("run_start", {
      file: options.file,
      logPath,
      chatCount: chats.length,
      run: options.run === true,
      transferOwner: ownershipTransferEnabled,
      passwordAvailable: env.password !== null,
      delayMs,
      targetUser: uniqueDivine.id,
      targetBot: saiBot.id,
    })

    try {
      for (const [i, chat] of chats.entries()) {
        printJsonEvent("chat_start", {
          index: i + 1,
          total: chats.length,
          chatId: chat.chatId,
          title: chat.title,
        })

        try {
          await applyTargetPlan({
            client,
            chat,
            targetUserId: saiBot.id,
            targetPeer: saiBot.handle,
            rights: saiBotAdminRights,
            actionPrefix: "bot",
            run: options.run === true,
          })
          await applyTargetPlan({
            client,
            chat,
            targetUserId: uniqueDivine.id,
            targetPeer: uniqueDivine.handle,
            rights: ownerTargetAdminRights,
            actionPrefix: "owner_target",
            run: options.run === true,
          })
          await maybeTransferOwner({
            client,
            env,
            chat,
            targetUser: uniqueDivine.handle,
            enabled: ownershipTransferEnabled,
            run: options.run === true,
          })
          printJsonEvent("chat_complete", {
            chatId: chat.chatId,
          })
        } catch (error) {
          printJsonEvent("chat_error", {
            chatId: chat.chatId,
            errorName: error instanceof Error ? error.name : "UnknownError",
            errorMessage:
              error instanceof Error ? error.message : String(error),
          })
        }

        if (delayMs > 0 && i < chats.length - 1) {
          printJsonEvent("chat_delay", {
            chatId: chat.chatId,
            delayMs,
          })
          await sleep(delayMs)
        }
      }

      printJsonEvent("run_complete", {
        file: options.file,
        logPath,
        chatCount: chats.length,
      })
    } finally {
      activeJsonLogPath = null
    }
  })
}

function getCredentialsFromProfileAddOptions(
  options: ProfileAddOptions,
): TelegramCredentials | null {
  if (options.apiId && options.apiHash && options.sessionString) {
    return {
      apiId: Number(options.apiId),
      apiHash: options.apiHash,
      sessionString: options.sessionString,
    }
  }
  return null
}

function credentialValuesFromProfileAddOptions(
  options: ProfileAddOptions,
): TelegramCredentialValues {
  return {
    TELEGRAM_API_ID: options.apiId,
    TELEGRAM_API_HASH: options.apiHash,
    TELEGRAM_SESSION_STRING: options.sessionString,
    TELEGRAM_PASSWORD: options.password,
  }
}

function profileFieldsFromCredentialValues(values: TelegramCredentialValues) {
  return {
    apiId: values.TELEGRAM_API_ID ? Number(values.TELEGRAM_API_ID) : null,
    apiHash: values.TELEGRAM_API_HASH ?? "",
    sessionString: values.TELEGRAM_SESSION_STRING ?? "",
    password: values.TELEGRAM_PASSWORD ?? "",
  }
}

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

function validateLoginOptions(options: ProfileAddOptions): void {
  if (options.qr && options.phone) {
    throw new Error("--qr and --phone are mutually exclusive")
  }
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

async function getCredentialValuesFromSourceOption(
  options: ProfileAddOptions,
): Promise<TelegramCredentialValues | null> {
  if (!options.src) {
    return null
  }
  const index = Number(options.src)
  if (!Number.isInteger(index) || index < 1) {
    throw new Error("--src must be a 1-based source number")
  }

  const report = await discoveryReport()
  const candidate = report.candidates[index - 1]
  if (!candidate) {
    throw new Error(`Unknown discovery source: ${options.src}`)
  }
  return candidate.values
}

async function inputFromProfileAddOptions(
  options: ProfileAddOptions,
): Promise<ProfileAddInput> {
  validateLoginOptions(options)
  const credentials = getCredentialsFromProfileAddOptions(options)
  const sourceValues = credentials
    ? null
    : await getCredentialValuesFromSourceOption(options)
  const optionValues = credentialValuesFromProfileAddOptions(options)
  const profileFields = credentials
    ? {
        apiId: credentials.apiId,
        apiHash: credentials.apiHash,
        sessionString: credentials.sessionString,
        password: optionValues.TELEGRAM_PASSWORD ?? "",
      }
    : profileFieldsFromCredentialValues(
        mergeCredentialValues(sourceValues, optionValues),
      )

  if (options.login && !profileFields.sessionString) {
    if (!profileFields.apiId || !profileFields.apiHash) {
      throw new Error(
        "--login requires apiId and apiHash from flags, --src, env, or .env discovery",
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

  const completeProfileFields: TelegramCredentials = {
    apiId: profileFields.apiId,
    apiHash: profileFields.apiHash,
    sessionString: profileFields.sessionString,
  }
  const client = await createMtcuteClient({
    apiId: completeProfileFields.apiId,
    apiHash: completeProfileFields.apiHash,
    session: completeProfileFields.sessionString,
  })
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
  const profiles = Object.values(config.profiles)
  if (profiles.length === 0) {
    console.log("No profiles configured.")
    return
  }
  for (const profile of profiles) {
    const active = config.activeProfile === profile.name ? "*" : " "
    console.log(
      `${active} ${profile.name}\t${profile.handle ?? ""}\t${profile.userId}\t${profile.displayName}`,
    )
  }
}

async function runProfileShow(name: string): Promise<void> {
  const config = await readConfig()
  const profile = getProfile(config, name)
  if (!profile) {
    throw new Error(`Unknown profile: ${name}`)
  }
  console.log(JSON.stringify(redactedProfile(profile), null, 2))
}

async function runProfileAdd(options: ProfileAddOptions): Promise<void> {
  const input = await inputFromProfileAddOptions(options)
  const config = await addProfile(input)
  const active = config.activeProfile === input.name ? "active" : "saved"
  console.log(`profile_${active}: ${input.name}`)
  console.log(`user_id: ${input.userId}`)
  console.log(`handle: ${input.handle ?? ""}`)
  console.log(`telegram_password: ${input.password ? "stored" : "missing"}`)
  console.log(
    "ownership_transfer: profile password or TELEGRAM_PASSWORD enables transfer",
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
  console.log("  tg-sai profile add --name <name> --src <number>")
}

export function createProgram(): Command {
  const program = new Command()

  program
    .name("tg-sai")
    .description("Audit and manage Sai Telegram chat admin state")
    .version("0.1.0")
    .helpOption("--help", "display help for command")

  program.action(() => {
    program.help({ error: false })
  })

  program
    .command("health")
    .description("Read-only preflight for the local tg-sai setup")
    .action(async () => {
      await runHealth()
    })

  program
    .command("audit")
    .description("Read-only audit for one chat and target user")
    .option("--profile <name>", "Profile name to use for Telegram credentials")
    .requiredOption(
      "--chat <id>",
      "Telegram chat ID or username",
      parsePeer,
    )
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
    .requiredOption(
      "--chat <id>",
      "Telegram chat ID or username",
      parsePeer,
    )
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
    .requiredOption(
      "--chat <id>",
      "Telegram chat ID or username",
      parsePeer,
    )
    .requiredOption(
      "--target-user <peer>",
      "Target user ID or username",
      parsePeer,
    )
    .option("--run", "Execute add/promote actions; default is dry-run")
    .action(async (options: AddOwnerOptions) => {
      await runAddOwner(options)
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
      "Skip ownership transfer even when TELEGRAM_PASSWORD is available",
    )
    .action(async (options: SaiBootstrapOptions) => {
      await runSaiBootstrap(options)
    })

  const profile = program
    .command("profile")
    .description("Manage local Telegram session profiles")

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

  profile
    .command("add")
    .description("Add or update a Telegram session profile")
    .requiredOption("--name <name>", "Profile name")
    .option("--api-id <id>", "Telegram API ID")
    .option("--api-hash <hash>", "Telegram API hash")
    .option("--session-string <value>", "Telegram session string")
    .option(
      "--password <value>",
      "Telegram 2FA password for ownership transfer",
    )
    .option("--src <number>", "Use a numbered source from profile find")
    .option("--login", "Generate a session string when missing")
    .option("--qr", "Use QR login with --login")
    .option("--phone", "Use phone login with --login")
    .option("--handle <handle>", "Resolved Telegram handle")
    .option("--user-id <id>", "Resolved Telegram user ID")
    .option("--display-name <name>", "Resolved Telegram display name")
    .action(async (options: ProfileAddOptions) => {
      await runProfileAdd(options)
    })

  profile
    .command("show")
    .description("Show one configured profile")
    .argument("<name>", "Profile name")
    .action(async (name: string) => {
      await runProfileShow(name)
    })

  profile
    .command("use")
    .description("Set the active profile")
    .argument("<name>", "Profile name")
    .action(async (name: string) => {
      await setActiveProfile(name)
      console.log(`active_profile: ${name}`)
    })

  profile
    .command("remove")
    .description("Remove one configured profile")
    .argument("<name>", "Profile name")
    .action(async (name: string) => {
      await removeProfile(name)
      console.log(`removed_profile: ${name}`)
    })

  return program
}

createProgram()
  .parseAsync()
  .catch((error) => {
    console.error(error)
    process.exit(1)
  })
