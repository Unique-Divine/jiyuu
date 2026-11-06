import { UNKNOWN_CLOUD_OPERATION } from "./provisioningCloud"
import { safeProvisionError } from "./provisioning"
import type { BackgroundQueue } from "./state/background"
import type { HoldSource, HoldStore } from "./state/holds"

/** Custody status report, tg-sai worker to Deal Desk (epic 18 contract). */
export interface CustodyStatusReport {
  accountId: string
  workerVersion: string
  observedAt: string
  holds: {
    group: string
    untilMs: number
    source: HoldSource
    method: string
    waitS: number | null
  }[]
  addsToday: number
  background: { pending: number; nextAt: string | null }
}

/** Reports go out at least this often, changed or not. */
export const CUSTODY_STATUS_INTERVAL_MS = 5 * 60 * 1000
/** An unsupported `custody-status` op is logged at most this often. */
export const UNKNOWN_OP_LOG_MS = 60 * 60 * 1000

/** Builds a report from saved holds and the background queue. Throws when either is unreadable. */
export function buildCustodyStatus(input: {
  accountId: number
  workerVersion: string
  holds: HoldStore
  queue: BackgroundQueue
  nowMs: number
}): CustodyStatusReport {
  const holds = input.holds
    .activeHolds(input.accountId, input.nowMs)
    .map((hold) => ({
      group: hold.grp,
      untilMs: hold.untilMs,
      source: hold.source,
      method: hold.method,
      waitS: hold.waitS,
    }))
  const summary = input.queue.summary()
  return {
    accountId: String(input.accountId),
    workerVersion: input.workerVersion,
    observedAt: new Date(input.nowMs).toISOString(),
    holds,
    addsToday: input.queue.addsToday(input.nowMs),
    background: {
      pending: summary.pending,
      nextAt:
        summary.pending && summary.nextAtMs !== null
          ? new Date(summary.nextAtMs).toISOString()
          : null,
    },
  }
}

/** The parts of a report whose change triggers a send. */
export function custodyStatusKey(report: CustodyStatusReport): string {
  return JSON.stringify({
    holds: report.holds.map((h) => [h.group, h.untilMs, h.source]),
    addsToday: report.addsToday,
    background: report.background,
  })
}

/** Log sink: one short constant message plus attrs (telemetry `log`). */
export type StatusLog = (
  level: "info" | "warn",
  msg: string,
  attrs?: Record<string, unknown>,
) => void

/**
 * Sends a custody status report when holds or background counts change, and
 * at least every `CUSTODY_STATUS_INTERVAL_MS`. `tick` never throws: a failed
 * build or send is logged (an unsupported op at most once an hour) and the
 * next tick tries again.
 */
export class CustodyStatusReporter {
  private lastKey: string | null = null
  private lastSentMs = -Infinity
  private lastUnknownLogMs = -Infinity
  private readonly intervalMs: number

  constructor(
    private readonly options: {
      build: (nowMs: number) => CustodyStatusReport
      send: (report: CustodyStatusReport) => Promise<unknown>
      log?: StatusLog
      intervalMs?: number
    },
  ) {
    this.intervalMs = options.intervalMs ?? CUSTODY_STATUS_INTERVAL_MS
  }

  /** Returns whether a report was sent. */
  async tick(nowMs: number = Date.now()): Promise<boolean> {
    let report: CustodyStatusReport
    try {
      report = this.options.build(nowMs)
    } catch (error) {
      this.log("warn", "custody status unavailable", {
        error: safeProvisionError(error),
      })
      return false
    }
    const key = custodyStatusKey(report)
    if (key === this.lastKey && nowMs - this.lastSentMs < this.intervalMs)
      return false
    try {
      await this.options.send(report)
    } catch (error) {
      // Retry on the next change or after a full interval, so a failing
      // CLI is not spawned on every tick.
      this.lastSentMs = nowMs
      this.lastKey = key
      if ((error as { code?: unknown })?.code === UNKNOWN_CLOUD_OPERATION) {
        if (nowMs - this.lastUnknownLogMs >= UNKNOWN_OP_LOG_MS) {
          this.lastUnknownLogMs = nowMs
          this.log("warn", "deal desk lacks custody-status")
        }
        return false
      }
      this.log("warn", "custody status send failed", {
        error: safeProvisionError(error),
      })
      return false
    }
    this.lastKey = key
    this.lastSentMs = nowMs
    return true
  }

  private log(
    level: "info" | "warn",
    msg: string,
    attrs?: Record<string, unknown>,
  ): void {
    try {
      this.options.log?.(level, msg, attrs)
    } catch {
      // Logging never stops the worker.
    }
  }
}
