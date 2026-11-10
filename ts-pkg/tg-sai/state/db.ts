import { Database } from "bun:sqlite"
import { chmodSync, closeSync, mkdirSync, openSync } from "node:fs"
import { dirname, join } from "node:path"

import { configDir, type ProfileStoreEnv } from "../profiles.ts"
import {
  applyMigrations,
  applyPragmas,
  type Migration,
} from "../telemetry/db.ts"
// Embedded at bundle/run time so the published package carries the schema.
import floodHoldMigrationSql from "./migrations/0001-flood-hold.sql" with {
  type: "text",
}
import backgroundAddMigrationSql from "./migrations/0002-background-add.sql" with {
  type: "text",
}
import backgroundPrivacyMigrationSql from "./migrations/0003-background-privacy.sql" with {
  type: "text",
}
import memberAddMigrationSql from "./migrations/0004-member-add.sql" with {
  type: "text",
}
import memberAddUncertainMigrationSql from "./migrations/0005-member-add-uncertain.sql" with {
  type: "text",
}
import poolFillMigrationSql from "./migrations/0006-pool-fill.sql" with {
  type: "text",
}

export const STATE_DB_FILE = "state.db"
export const STATE_DIR_MODE = 0o700
export const STATE_FILE_MODE = 0o600

/**
 * Migrations for `state.db`. Separate from telemetry: state must work with
 * telemetry off, and its history table lives in its own file.
 */
export const STATE_MIGRATIONS: readonly Migration[] = [
  { filename: "0001-flood-hold.sql", sql: floodHoldMigrationSql },
  { filename: "0002-background-add.sql", sql: backgroundAddMigrationSql },
  {
    filename: "0003-background-privacy.sql",
    sql: backgroundPrivacyMigrationSql,
  },
  { filename: "0004-member-add.sql", sql: memberAddMigrationSql },
  {
    filename: "0005-member-add-uncertain.sql",
    sql: memberAddUncertainMigrationSql,
  },
  { filename: "0006-pool-fill.sql", sql: poolFillMigrationSql },
]

export function stateDbPath(env: ProfileStoreEnv = process.env): string {
  return join(configDir(env), STATE_DB_FILE)
}

function restrictMode(path: string, mode: number): void {
  try {
    chmodSync(path, mode)
  } catch {
    // Not our directory (or a filesystem without modes); the open decides.
  }
}

/**
 * Creates the parent directory (0700) and the database file (0600) before
 * SQLite opens it, so the file never exists with wider permissions. SQLite
 * gives `-wal` and `-shm` the database file's mode.
 */
export function prepareStateFile(path: string): void {
  const dir = dirname(path)
  mkdirSync(dir, { recursive: true, mode: STATE_DIR_MODE })
  restrictMode(dir, STATE_DIR_MODE)
  closeSync(openSync(path, "a", STATE_FILE_MODE))
  restrictMode(path, STATE_FILE_MODE)
}

/**
 * Opens (creating if needed) the state database at `path`: WAL,
 * `busy_timeout` 5000 ms, and `STATE_MIGRATIONS`. Throws on failure; use
 * `HoldStore.open` for a store that reports `unavailable` instead.
 */
export function openStateDb(
  path: string,
  migrations: readonly Migration[] = STATE_MIGRATIONS,
): Database {
  if (path !== ":memory:") prepareStateFile(path)
  const db = new Database(path, { create: true, strict: true })
  try {
    applyPragmas(db)
    applyMigrations(db, migrations)
    return db
  } catch (error) {
    db.close()
    throw error
  }
}
