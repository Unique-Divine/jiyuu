import { createHash, randomUUID } from "node:crypto"
import type {
  Progress,
  ProvisionCloud,
  ProvisionRequest,
  Setup,
} from "./provisioning"
import { validateRequest } from "./provisioning"
import type { CustodyStatusReport } from "./custodyStatus"

export type CloudOperation =
  | "prepare"
  | "view"
  | "list"
  | "pending"
  | "claim"
  | "progress"
  | "resolve"
  | "custody-status"
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
      "ready",
      "cancelled",
    ].includes(setup.state)
  )
    throw new Error("Cloud returned invalid setup state")
  return setup
}
export class CliProvisionCloud implements ProvisionCloud {
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

/**
 * Whether Deal Desk's stderr says it does not know `operation`: clap's
 * `invalid value '<op>' for '<ACTION>'` or `unrecognized subcommand`, or a
 * GraphQL server without the field.
 */
export function isUnknownOperationOutput(
  operation: CloudOperation,
  stderr: string,
): boolean {
  return (
    stderr.includes(`invalid value '${operation}'`) ||
    /unrecognized subcommand|unknown field|cannot query field/i.test(stderr)
  )
}

export function dealDeskRequestKey(
  operation: CloudOperation,
  input: Record<string, unknown>,
): string {
  // The same payload can be a new intent after operator recovery or an explicit
  // creation rejection. Receipts must only deduplicate one invocation.
  if (operation === "claim" || operation === "progress")
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
  if (operation !== "view" && operation !== "list" && operation !== "pending")
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
