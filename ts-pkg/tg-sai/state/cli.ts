/**
 * `tg-sai holds`: inspect and override the flood holds in `state.db`.
 * Writes are dry runs unless `--run` is given. `set` records a `manual`
 * hold with MAX semantics, so it can extend a hold but never shorten it;
 * `clear` is the only way to end a hold early.
 */
import { Database } from "bun:sqlite"
import type { Command } from "commander"
import { existsSync } from "node:fs"

import type { ProfileStoreEnv } from "../profiles"
import {
  isoTime,
  parseAccountId,
  parsePositiveInt,
  toonObject,
  toonTable,
  type Cell,
} from "../telemetry/cli"
import { openStateDb, stateDbPath } from "./db"
import {
  HoldStore,
  MAX_HOLD_MS,
  holdUntil,
  remainingSeconds,
  type FloodHold,
  type HoldGroup,
} from "./holds"

// ---------------------------------------------------------------------------
// Option parsing

const NAMED_GROUPS: readonly HoldGroup[] = [
  "invite",
  "create",
  "resolve",
  "send",
]
const METHOD_NAME = /^[A-Za-z][\w]*(?:\.[A-Za-z][\w]*)+$/
const REASON_CODE = /^[A-Za-z0-9_.:-]{1,64}$/

/** Parses `invite|create|resolve|send|method:<tl.method>`. */
export function parseHoldGroup(value: string): HoldGroup {
  const trimmed = value.trim()
  if ((NAMED_GROUPS as readonly string[]).includes(trimmed)) {
    return trimmed as HoldGroup
  }
  if (trimmed.startsWith("method:")) {
    const method = trimmed.slice("method:".length)
    if (METHOD_NAME.test(method)) return `method:${method}`
  }
  throw new Error(
    `--group must be invite, create, resolve, send, or method:<name> ` +
      `(e.g. method:users.getFullUser), got "${value}"`,
  )
}

/** Parses an ISO 8601 time with a zone; must be after `nowMs`. */
export function parseUntil(value: string, nowMs: number): number {
  const hasZone = /(?:Z|[+-]\d{2}:?\d{2})$/i.test(value.trim())
  const ms = Date.parse(value)
  if (!hasZone || !Number.isFinite(ms)) {
    throw new Error(
      `--until must be an ISO 8601 time with a zone, ` +
        `e.g. 2026-11-07T12:00:00Z, got "${value}"`,
    )
  }
  if (ms <= nowMs) {
    throw new Error(`--until must be in the future, got "${value}"`)
  }
  return ms
}

export function parseReasonCode(value: string): string {
  if (!REASON_CODE.test(value)) {
    throw new Error(
      `--reason-code must be 1-64 of [A-Za-z0-9_.:-], got "${value}"`,
    )
  }
  return value
}

/** Default `method` for a manual hold: the group's own method, or `manual`. */
export function defaultMethod(grp: HoldGroup): string {
  return grp.startsWith("method:") ? grp.slice("method:".length) : "manual"
}

/** `request_id` for a manual hold: `manual:<code>`, or null without one. */
export function manualRequestId(reasonCode: string | undefined): string | null {
  return reasonCode === undefined ? null : `manual:${reasonCode}`
}

// ---------------------------------------------------------------------------
// Projections

export const HOLD_FIELDS = [
  "account",
  "group",
  "until",
  "remaining_s",
  "method",
  "wait_s",
  "source",
] as const

/** One hold as printed by `holds list`; `remaining_s` is 0 once ended. */
export function holdProjection(
  hold: FloodHold,
  nowMs: number,
): Record<string, Cell> {
  return {
    account: hold.accountId,
    group: hold.grp,
    until: isoTime(hold.untilMs),
    remaining_s:
      hold.untilMs > nowMs ? remainingSeconds(hold.untilMs, nowMs) : 0,
    method: hold.method,
    wait_s: hold.waitS,
    source: hold.source,
  }
}

interface HoldRow {
  account_id: string
  grp: string
  until_ms: number
  method: string
  wait_s: number | null
  source: string
  request_id: string | null
  created_ms: number
  updated_ms: number
}

/** Every stored hold, ended ones included, latest end first. */
export function allHolds(db: Database, accountId?: string): FloodHold[] {
  const rows =
    accountId === undefined
      ? db
          .query<HoldRow, []>(
            "SELECT * FROM flood_hold ORDER BY until_ms DESC, account_id, grp",
          )
          .all()
      : db
          .query<HoldRow, [string]>(
            "SELECT * FROM flood_hold WHERE account_id = ? " +
              "ORDER BY until_ms DESC, grp",
          )
          .all(accountId)
  return rows.map((row) => ({
    accountId: row.account_id,
    grp: row.grp as HoldGroup,
    untilMs: row.until_ms,
    method: row.method,
    waitS: row.wait_s,
    source: row.source as FloodHold["source"],
    requestId: row.request_id,
    createdMs: row.created_ms,
    updatedMs: row.updated_ms,
  }))
}

/** The stored hold on `(accountId, grp)`, ended or not. */
export function storedHold(
  db: Database,
  accountId: string,
  grp: HoldGroup,
): FloodHold | null {
  return allHolds(db, accountId).find((hold) => hold.grp === grp) ?? null
}

/**
 * What `recordHold` would store, given the current row: the later end
 * time wins, capped at one year from `nowMs`.
 */
export function plannedUntil(
  requestedUntilMs: number,
  existing: FloodHold | null,
  nowMs: number,
): { untilMs: number; extends: boolean } {
  const capped = Math.min(Math.round(requestedUntilMs), nowMs + MAX_HOLD_MS)
  if (existing && existing.untilMs >= capped) {
    return { untilMs: existing.untilMs, extends: false }
  }
  return { untilMs: capped, extends: true }
}

// ---------------------------------------------------------------------------
// Command wiring

export interface HoldsCliContext {
  env?: ProfileStoreEnv
  /** Overrides `stateDbPath(env)`. */
  dbPath?: string
  now?: () => number
  stdout?: (text: string) => void
  stderr?: (text: string) => void
}

interface ResolvedContext {
  path: string
  now: () => number
  out: (text: string) => void
  err: (text: string) => void
}

function resolveContext(ctx: HoldsCliContext): ResolvedContext {
  return {
    path: ctx.dbPath ?? stateDbPath(ctx.env ?? process.env),
    now: ctx.now ?? Date.now,
    out: ctx.stdout ?? ((text) => process.stdout.write(`${text}\n`)),
    err: ctx.stderr ?? ((text) => process.stderr.write(`${text}\n`)),
  }
}

/**
 * Runs `fn` on a read-only handle, or on `null` when the database does not
 * exist yet (no hold has ever been recorded). Never creates the file.
 */
function withReadOnlyDb<T>(
  ctx: ResolvedContext,
  fn: (db: Database | null) => T,
): T {
  if (!existsSync(ctx.path)) return fn(null)
  const db = new Database(ctx.path, { readonly: true, strict: true })
  try {
    db.run("PRAGMA busy_timeout = 5000")
    return fn(db)
  } finally {
    db.close()
  }
}

/** Opens the database for writing, creating and migrating it if needed. */
function withWritableStore<T>(
  ctx: ResolvedContext,
  fn: (store: HoldStore, db: Database) => T,
): T {
  const db = openStateDb(ctx.path)
  const store = HoldStore.fromDb(db)
  try {
    return fn(store, db)
  } finally {
    store.close()
  }
}

/** True when no option of `command` was given on the command line. */
function calledBare(command: Command): boolean {
  return command.options.every(
    (option) => command.getOptionValueSource(option.attributeName()) !== "cli",
  )
}

function printHelp(command: Command): void {
  command.outputHelp()
}

/** Throws a commander usage error so the CLI exits 1 with the message. */
function usageError(command: Command, error: unknown): never {
  const message = error instanceof Error ? error.message : String(error)
  command.error(`error: ${message}`, { exitCode: 1, code: "tg-sai.holds" })
}

function parseOrFail<T>(command: Command, fn: () => T): T {
  try {
    return fn()
  } catch (error) {
    usageError(command, error)
  }
}

interface ListCliOptions {
  account?: string
  all?: boolean
  json?: boolean
}

interface SetCliOptions {
  account?: string
  group?: string
  until?: string
  waitS?: string
  method?: string
  reasonCode?: string
  run?: boolean
  json?: boolean
}

interface ClearCliOptions {
  account?: string
  group?: string
  run?: boolean
  json?: boolean
}

function requireOption(
  command: Command,
  value: string | undefined,
  flag: string,
): string {
  if (value === undefined) usageError(command, `${flag} is required`)
  return value
}

/**
 * Registers `tg-sai holds` and its subcommands on `program`. `context` is
 * for tests; production uses the environment, clock, and stdio.
 */
export function registerHoldsCommand(
  program: Command,
  context: HoldsCliContext = {},
): Command {
  const ctx = resolveContext(context)
  if (context.stdout || context.stderr) {
    program.configureOutput({
      writeOut: (text) => ctx.out(text.replace(/\n$/, "")),
      writeErr: (text) => ctx.err(text.replace(/\n$/, "")),
    })
  }

  const holds = program
    .command("holds")
    .description("Inspect and override saved Telegram flood holds (state.db)")
    .addHelpText(
      "after",
      `
Examples:
  tg-sai holds list
  tg-sai holds list --account 8274304175 --all --json
  tg-sai holds set --account 8274304175 --group invite --wait-s 3600
  tg-sai holds set --account 8274304175 --group send \\
    --until 2026-11-07T12:00:00Z --reason-code ops_pause --run
  tg-sai holds clear --account 8274304175 --group invite --run

set and clear are dry runs without --run. set can only extend a hold.
Groups: invite, create, resolve, send, method:<tl.method>.
`,
    )
    .action(() => {
      printHelp(holds)
    })

  holds
    .command("list")
    .description("Active holds, latest end first (all stored with --all)")
    .option("--account <id>", "Only this Telegram account id")
    .option("--all", "Include holds that have already ended")
    .option("--json", "Print JSON")
    .action((options: ListCliOptions, command: Command) => {
      const accountId =
        options.account === undefined
          ? undefined
          : String(parseOrFail(command, () => parseAccountId(options.account!)))
      const nowMs = ctx.now()
      const rows = withReadOnlyDb(ctx, (db) => {
        if (!db) return []
        const found = options.all
          ? allHolds(db, accountId)
          : HoldStore.fromDb(db).activeHolds(accountId, nowMs)
        return found.map((hold) => holdProjection(hold, nowMs))
      })
      if (options.json) ctx.out(JSON.stringify(rows, null, 2))
      else ctx.out(toonTable("holds", HOLD_FIELDS, rows))
    })

  holds
    .command("set")
    .description("Record a manual hold; extends a hold, never shortens one")
    .option("--account <id>", "Telegram account id")
    .option(
      "--group <group>",
      "invite, create, resolve, send, or method:<tl.method>",
    )
    .option("--until <iso>", "End time, ISO 8601 with zone (UTC recommended)")
    .option("--wait-s <n>", "Hold for N seconds plus the usual margin")
    .option("--method <name>", "Method to record (default: from the group)")
    .option("--reason-code <code>", "Saved as request_id manual:<code>")
    .option("--run", "Write the hold (default is a dry run)")
    .option("--json", "Print JSON")
    .addHelpText(
      "after",
      `
Exactly one of --until or --wait-s. Without --run nothing is written.
`,
    )
    .action((options: SetCliOptions, command: Command) => {
      if (calledBare(command)) {
        printHelp(command)
        return
      }
      const nowMs = ctx.now()
      const account = requireOption(command, options.account, "--account")
      const group = requireOption(command, options.group, "--group")
      const accountId = String(
        parseOrFail(command, () => parseAccountId(account)),
      )
      const grp = parseOrFail(command, () => parseHoldGroup(group))
      if ((options.until === undefined) === (options.waitS === undefined)) {
        usageError(command, "give exactly one of --until or --wait-s")
      }
      const waitS =
        options.waitS === undefined
          ? null
          : parseOrFail(command, () =>
              parsePositiveInt(options.waitS!, "--wait-s"),
            )
      const requestedUntilMs =
        options.until === undefined
          ? holdUntil(nowMs, waitS ?? 0)
          : parseOrFail(command, () => parseUntil(options.until!, nowMs))
      const method = options.method ?? defaultMethod(grp)
      const requestId = manualRequestId(
        options.reasonCode === undefined
          ? undefined
          : parseOrFail(command, () => parseReasonCode(options.reasonCode!)),
      )

      const write = (store: HoldStore | null, db: Database | null) => {
        const existing = db ? storedHold(db, accountId, grp) : null
        const active = existing && existing.untilMs > nowMs ? existing : null
        const plan = plannedUntil(requestedUntilMs, active, nowMs)
        const stored = store
          ? store.recordHold({
              accountId,
              grp,
              method,
              source: "manual",
              waitS,
              untilMs: requestedUntilMs,
              requestId,
              nowMs,
            })
          : null
        const untilMs = stored?.untilMs ?? plan.untilMs
        return {
          mode: store ? "run" : "dry_run",
          path: ctx.path,
          account: accountId,
          group: grp,
          requested_until: isoTime(requestedUntilMs),
          until: isoTime(untilMs),
          remaining_s: remainingSeconds(untilMs, nowMs),
          method: plan.extends ? method : (active?.method ?? method),
          wait_s: plan.extends ? waitS : (active?.waitS ?? null),
          source: plan.extends ? "manual" : (active?.source ?? "manual"),
          request_id: plan.extends ? requestId : (active?.requestId ?? null),
          previous_until: isoTime(active?.untilMs),
          changed: plan.extends,
        } satisfies Record<string, Cell>
      }

      const result = options.run
        ? withWritableStore(ctx, (store, db) => write(store, db))
        : withReadOnlyDb(ctx, (db) => write(null, db))
      if (options.json) ctx.out(JSON.stringify(result, null, 2))
      else ctx.out(toonObject(result))
      if (!result.changed) {
        ctx.err(
          "An existing hold ends later; MAX semantics keep it unchanged.",
        )
      }
      if (!options.run) ctx.err("Dry run: nothing written. Add --run to write.")
    })

  holds
    .command("clear")
    .description("Remove one hold so the account can retry the group now")
    .option("--account <id>", "Telegram account id")
    .option(
      "--group <group>",
      "invite, create, resolve, send, or method:<tl.method>",
    )
    .option("--run", "Delete the hold (default is a dry run)")
    .option("--json", "Print JSON")
    .action((options: ClearCliOptions, command: Command) => {
      if (calledBare(command)) {
        printHelp(command)
        return
      }
      const nowMs = ctx.now()
      const account = requireOption(command, options.account, "--account")
      const group = requireOption(command, options.group, "--group")
      const accountId = String(
        parseOrFail(command, () => parseAccountId(account)),
      )
      const grp = parseOrFail(command, () => parseHoldGroup(group))
      const existing = withReadOnlyDb(ctx, (db) =>
        db ? storedHold(db, accountId, grp) : null,
      )
      let removed = false
      if (options.run && existing) {
        removed = withWritableStore(ctx, (store) =>
          store.clearHold(accountId, grp),
        )
      }
      const result: Record<string, Cell> = {
        mode: options.run ? "run" : "dry_run",
        path: ctx.path,
        account: accountId,
        group: grp,
        found: existing !== null,
        removed,
        until: isoTime(existing?.untilMs),
        remaining_s:
          existing && existing.untilMs > nowMs
            ? remainingSeconds(existing.untilMs, nowMs)
            : 0,
        source: existing?.source ?? null,
      }
      if (options.json) ctx.out(JSON.stringify(result, null, 2))
      else ctx.out(toonObject(result))
      if (!existing) ctx.err(`No hold stored for ${accountId} ${grp}.`)
      else if (!options.run) {
        ctx.err("Dry run: nothing removed. Add --run to remove.")
      }
    })

  return holds
}
