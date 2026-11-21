import { Database } from "bun:sqlite"
import { join } from "node:path"
import type { DealChatPoolReceipt } from "../dealChatPoolExecutor"
import { configDir } from "../profiles"
import { applyMigrations, applyPragmas } from "../telemetry/db"
import { prepareStateFile } from "./db"
import initialSchema from "./migrations/0001-deal-chat-pool-outbox.sql" with { type: "text" }

/**
 * The in-flight result stays on disk until Deal Desk acknowledges it. Keep
 * it separate from state.db so a worker rollback can still open that DB.
 */
export class DealChatPoolOutbox {
  static open(path = join(configDir(), "deal-chat-pool-outbox.db")) {
    if (path !== ":memory:") prepareStateFile(path)
    const db = new Database(path)
    try {
      applyPragmas(db)
      applyMigrations(db, [
        { filename: "0001-deal-chat-pool-outbox.sql", sql: initialSchema },
      ])
      return new DealChatPoolOutbox(db)
    } catch (error) {
      db.close()
      throw error
    }
  }

  private constructor(private readonly db: Database) {}

  get(accountId: number): DealChatPoolReceipt | null {
    const row = this.db
      .query<{ result_json: string }, [number]>(
        "SELECT result_json FROM managed_deal_chat_pool_receipt WHERE account_id = ?",
      )
      .get(accountId)
    return row ? (JSON.parse(row.result_json) as DealChatPoolReceipt) : null
  }

  save(accountId: number, receipt: DealChatPoolReceipt): void {
    this.db.run(
      "INSERT INTO managed_deal_chat_pool_receipt (account_id, result_json) VALUES (?, ?) " +
        "ON CONFLICT(account_id) DO UPDATE SET result_json = excluded.result_json",
      [accountId, JSON.stringify(receipt)],
    )
  }

  clear(accountId: number): void {
    this.db.run(
      "DELETE FROM managed_deal_chat_pool_receipt WHERE account_id = ?",
      [accountId],
    )
  }

  close(): void {
    this.db.close()
  }
}
