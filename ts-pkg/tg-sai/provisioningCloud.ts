import { createHash, randomUUID } from "node:crypto"
import type {
  Progress,
  ProvisionCloud,
  ProvisionRequest,
  Setup,
} from "./provisioning"
import { validateRequest } from "./provisioning"
import type { CustodyStatusReport } from "./custodyStatus"
import type {
  PoolCloud,
  PoolFailure,
  PoolRegistration,
  PoolStatusReply,
} from "./poolFiller"

export type CloudOperation =
  | "prepare"
  | "view"
  | "list"
  | "pending"
  | "claim"
  | "progress"
  | "resolve"
  | "custody-status"
  | "pool-status"
  | "pool-register"
  | "pool-fail"
export type CloudCall = (
  operation: CloudOperation,
  input: Record<string, unknown>,
) => Promise<unknown>

export function cloudRequest(
  request: ProvisionRequest,
): Record<string, unknown> {
  validateRequest(request)
  return {
    requestId: request.requestId,
    custodyProfile: request.custodyProfile,
    title: request.title,
    botRightsProfile: request.botRightsProfile,
    dealIds: request.dealIds,
    expectedUserId: String(request.expectedUserId),
    participants: request.participants.map(String),
    ...(request.optionalParticipants === undefined
      ? {}
      : { optionalParticipants: request.optionalParticipants.map(String) }),
    admins: request.admins.map((admin) => ({
      userId: String(admin.userId),
      rightsProfile: admin.rightsProfile,
    })),
  }
}
function numericId(value: unknown): number {
  if (typeof value !== "string" && typeof value !== "number")
    throw new Error("Cloud returned invalid Telegram identity")
  const id = Number(value)
  if (!Number.isSafeInteger(id) || id === 0)
    throw new Error("Cloud returned invalid Telegram identity")
  return id
}
export function setupFromCloud(raw: unknown): Setup {
  if (!raw || typeof raw !== "object")
    throw new Error("Cloud returned invalid setup")
  const value = raw as Record<string, unknown>
  const setup = {
    ...value,
    expectedUserId: numericId(value.expectedUserId),
  } as unknown as Setup
  if (value.chatId !== undefined && value.chatId !== null)
    setup.chatId = numericId(value.chatId)
  else delete setup.chatId
  if (value.resolvedParticipants != null || value.resolvedAdmins != null) {
    if (
      !Array.isArray(value.resolvedParticipants) ||
      !Array.isArray(value.resolvedAdmins)
    )
      throw new Error("Cloud returned incomplete identity bindings")
    setup.binding = {
      participants: value.resolvedParticipants.map(numericId),
      admins: value.resolvedAdmins.map(numericId),
      ...(value.resolvedOptionalParticipants == null
        ? {}
        : {
            optionalParticipants: (
              value.resolvedOptionalParticipants as unknown[]
            ).map(numericId),
          }),
    }
  }
  validateRequest(setup)
  if (
    ![
      "prepared",
      "creating",
      "creation_unknown",
      "configuring",
      "incomplete",
      "awaiting_join",
      "ready",
      "cancelled",
    ].includes(setup.state)
  )
    throw new Error("Cloud returned invalid setup state")
  return setup
}
/** Checks a `pool-status` reply; throws when a field the filler reads is off. */
export function poolStatusFromCloud(raw: unknown): PoolStatusReply {
  const value = raw as Record<string, unknown> | null
  if (!value || typeof value !== "object")
    throw new Error("Cloud returned invalid pool status")
  if (typeof value.allowedNow !== "boolean")
    throw new Error("Cloud returned invalid pool status")
  const cap = value.cap
  if (
    cap !== null &&
    cap !== undefined &&
    !(typeof cap === "number" && Number.isSafeInteger(cap) && cap >= 0)
  )
    throw new Error("Cloud returned invalid pool cap")
  return {
    accountId: String(value.accountId ?? ""),
    allowedNow: value.allowedNow,
    reason: typeof value.reason === "string" ? value.reason : null,
    cap: typeof cap === "number" ? cap : null,
    ...(typeof value.remaining === "number"
      ? { remaining: value.remaining }
      : {}),
    ...(typeof value.needed === "number" ? { needed: value.needed } : {}),
  }
}

export class CliProvisionCloud implements ProvisionCloud, PoolCloud {
  constructor(private call: CloudCall = callDealDesk) {}
  async pending(input: {
    expectedUserId: number
    workerId: string
    limit: number
    after?: string
  }): Promise<{
    requests: Setup[]
    nextAfter: string | null
    blocked: { requestId: string; state: string; reason: string }[]
  }> {
    const raw = (await this.call("pending", {
      ...input,
      expectedUserId: String(input.expectedUserId),
    })) as {
      requests: unknown[]
      nextAfter?: string | null
      blocked?: { requestId: string; state: string; reason: string }[]
    }
    if (!Array.isArray(raw?.requests))
      throw new Error("Cloud returned invalid pending queue")
    return {
      requests: raw.requests.map(setupFromCloud),
      nextAfter: raw.nextAfter ?? null,
      blocked: raw.blocked ?? [],
    }
  }
  async prepare(request: ProvisionRequest): Promise<Setup> {
    return setupFromCloud(await this.call("prepare", cloudRequest(request)))
  }
  async view(requestId: string): Promise<Setup> {
    return setupFromCloud(await this.call("view", { requestId }))
  }
  async claim(requestId: string, workerId: string): Promise<Setup> {
    return setupFromCloud(await this.call("claim", { requestId, workerId }))
  }
  async progress(
    requestId: string,
    claimToken: string,
    progress: Progress,
  ): Promise<Setup> {
    const { binding, ...rest } = progress
    return setupFromCloud(
      await this.call("progress", {
        requestId,
        claimToken,
        ...rest,
        ...(progress.chatId === undefined
          ? {}
          : { chatId: String(progress.chatId) }),
        ...(binding
          ? {
              resolvedParticipants: binding.participants.map(String),
              resolvedAdmins: binding.admins.map(String),
              ...(binding.optionalParticipants === undefined
                ? {}
                : {
                    resolvedOptionalParticipants:
                      binding.optionalParticipants.map(String),
                  }),
            }
          : {}),
      }),
    )
  }
  /** Sends one custody status report; resolves to Deal Desk's reply. */
  async custodyStatus(report: CustodyStatusReport): Promise<unknown> {
    return this.call("custody-status", { ...report })
  }
  /** What to fill next for one owner account (`pool-status`). */
  async poolStatus(accountId: number): Promise<PoolStatusReply> {
    return poolStatusFromCloud(
      await this.call("pool-status", { accountId: String(accountId) }),
    )
  }
  /** Records a pool chat as `filling` or `available` (`pool-register`). */
  async poolRegister(input: PoolRegistration): Promise<unknown> {
    return this.call("pool-register", { ...input })
  }
  /** Reports a failed fill (`pool-fail`). */
  async poolFail(input: PoolFailure): Promise<unknown> {
    return this.call("pool-fail", { ...input })
  }
  async resolve(
    requestId: string,
    chatId: number,
    reason: string,
  ): Promise<Setup> {
    return setupFromCloud(
      await this.call("resolve", { requestId, chatId: String(chatId), reason }),
    )
  }
}

/** Code of `UnknownCloudOperationError`. */
export const UNKNOWN_CLOUD_OPERATION = "UNKNOWN_CLOUD_OPERATION"

/**
 * Deal Desk does not know the operation yet (older CLI or server), as
 * opposed to refusing its input. Carries no CLI output.
 */
export class UnknownCloudOperationError extends Error {
  readonly code = UNKNOWN_CLOUD_OPERATION
  constructor(readonly operation: CloudOperation) {
    super(`Deal Desk does not support chat-setups ${operation} yet`)
    this.name = "UnknownCloudOperationError"
  }
}

/** Code of `DealDeskRefusedError`. */
export const DEAL_DESK_REFUSED = "DEAL_DESK_REFUSED"

/**
 * Stable `extensions.code` values Deal Desk gives a pool operation that
 * refuses its input (pool contract, Refusals). The same input always gets
 * the same refusal.
 */
export const POOL_REFUSAL_CODES = [
  "POOL_NOT_OWNER",
  "POOL_INVALID_INPUT",
  "POOL_REFUSED_TRANSITION",
  "POOL_MISSING_RIGHTS",
  "POOL_STALE_VERIFIED_AT",
] as const
export type PoolRefusalCode = (typeof POOL_REFUSAL_CODES)[number]

/**
 * Deal Desk refused the input itself with one of `POOL_REFUSAL_CODES`, as
 * opposed to being down, busy, or rate limiting. Carries the code, never
 * CLI output.
 */
export class DealDeskRefusedError extends Error {
  readonly code = DEAL_DESK_REFUSED
  constructor(
    readonly operation: CloudOperation,
    readonly refusal: PoolRefusalCode,
  ) {
    super(`Deal Desk refused chat-setups ${operation}: ${refusal}`)
    this.name = "DealDeskRefusedError"
  }
}

const GRAPHQL_FAILURE = /GraphQL request failed \(HTTP \d{3}\): /

/**
 * The JSON value that starts at `text[start]`, or `undefined` when it does
 * not parse. Scans to the matching bracket, skipping string contents, so
 * text the CLI appends after the value (`. Request key: ...`) is ignored.
 */
function jsonValueAt(text: string, start: number): unknown {
  const open = text[start]
  if (open !== "[" && open !== "{") return undefined
  let depth = 0
  let inString = false
  for (let i = start; i < text.length; i++) {
    const c = text[i]
    if (inString) {
      if (c === "\\") i++
      else if (c === '"') inString = false
    } else if (c === '"') inString = true
    else if (c === "[" || c === "{") depth++
    else if (c === "]" || c === "}") {
      depth--
      if (depth === 0) {
        try {
          return JSON.parse(text.slice(start, i + 1))
        } catch {
          return undefined
        }
      }
    }
  }
  return undefined
}

/**
 * The pool refusal code in Deal Desk's stderr, or `null`. The CLI prints
 * `{"error": "GraphQL request failed (HTTP <n>): <errors JSON>..."}`, so the
 * errors array sits one level of escaping deep. Only `extensions.code` of
 * an entry in that array counts; a code in a message, output that does not
 * parse, and any error without one of `POOL_REFUSAL_CODES` (HTTP status,
 * rate limit, credential rotation, `Query capacity reached`, network,
 * database) are transient.
 */
export function refusalCode(stderr: string): PoolRefusalCode | null {
  const texts: string[] = [stderr]
  for (const line of stderr.split("\n")) {
    try {
      const error = (JSON.parse(line) as { error?: unknown } | null)?.error
      if (typeof error === "string") texts.push(error)
    } catch {
      // Not a JSON line; the raw text is tried as is.
    }
  }
  for (const text of texts) {
    const match = GRAPHQL_FAILURE.exec(text)
    if (!match) continue
    const errors = jsonValueAt(text, match.index + match[0].length)
    if (!Array.isArray(errors)) continue
    for (const entry of errors) {
      const code = (entry as { extensions?: { code?: unknown } } | null)
        ?.extensions?.code
      if (
        typeof code === "string" &&
        (POOL_REFUSAL_CODES as readonly string[]).includes(code)
      )
        return code as PoolRefusalCode
    }
  }
  return null
}

/** GraphQL mutation field Deal Desk sends for each mutating operation. */
export const CLOUD_MUTATION_FIELDS: Partial<Record<CloudOperation, string>> =
  {
    prepare: "chatSetupPrepare",
    claim: "chatSetupClaim",
    progress: "chatSetupProgress",
    resolve: "chatSetupResolve",
    "custody-status": "chatSetupCustodyStatus",
    // A query, but an older server reports it the same way.
    "pool-status": "poolStatus",
    "pool-register": "poolRegister",
    "pool-fail": "poolFail",
  }

/**
 * Whether Deal Desk's stderr says it does not know `operation` itself:
 * clap's `invalid value '<op>'` (an older CLI), or a GraphQL server that has
 * no such mutation (`Unknown field` or `Cannot query field` naming the
 * mutation field). An unknown field inside the input, such as a
 * `deny_unknown_fields` or input validation refusal, is a normal failure.
 */
export function isUnknownOperationOutput(
  operation: CloudOperation,
  stderr: string,
): boolean {
  if (stderr.includes(`invalid value '${operation}'`)) return true
  const field = CLOUD_MUTATION_FIELDS[operation]
  if (!field) return false
  return new RegExp(
    // Deal Desk prints errors as JSON, so a quote may be escaped.
    `(?:unknown field|cannot query field)\\s*\\\\?["'\`]?${field}\\b`,
    "i",
  ).test(stderr)
}

export function dealDeskRequestKey(
  operation: CloudOperation,
  input: Record<string, unknown>,
): string {
  // The same payload can be a new intent after operator recovery or an explicit
  // creation rejection. Receipts must only deduplicate one invocation.
  // A pool registration must be checked against the chat's current state
  // each time (an operator may have retired it since), not replayed from
  // an earlier receipt. Registering the same state again is a no-op.
  if (
    operation === "claim" ||
    operation === "progress" ||
    operation === "pool-register"
  )
    return `tg-provision:${operation}:${randomUUID()}`
  return `tg-provision:${createHash("sha256").update(JSON.stringify({ operation, input })).digest("hex")}`
}

/** Spawn argv directly; credentials stay in the existing Deal Desk client environment. */
export async function callDealDesk(
  operation: CloudOperation,
  input: Record<string, unknown>,
): Promise<unknown> {
  const key = dealDeskRequestKey(operation, input)
  const command = ["sai-deal-desk", "chat-setups", operation, "--json-stdin"]
  if (
    operation !== "view" &&
    operation !== "list" &&
    operation !== "pending" &&
    operation !== "pool-status"
  )
    command.push("--request-key", key)
  const child = Bun.spawn(command, {
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
  })
  child.stdin.write(JSON.stringify(input))
  child.stdin.end()
  const [stdout, stderr, exit] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ])
  if (exit !== 0 && isUnknownOperationOutput(operation, stderr))
    throw new UnknownCloudOperationError(operation)
  const refusal = exit !== 0 ? refusalCode(stderr) : null
  if (refusal) throw new DealDeskRefusedError(operation, refusal)
  if (exit !== 0)
    throw new Error(
      `Deal Desk chat-setups ${operation} failed; inspect its operator status separately`,
    )
  try {
    return JSON.parse(stdout)
  } catch {
    throw new Error("Deal Desk returned non-JSON setup response")
  }
}
