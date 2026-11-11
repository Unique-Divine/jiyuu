//! Plan and apply ordered, frozen SQL migrations for one SQLite database kind.
//!
//! Each migration advances the schema or data from the preceding version. The
//! migration-history table records the applied prefix and each SQL checksum.
//! A fresh database runs the entire sequence; an existing database runs only
//! its pending suffix. Applied SQL is not replayed on a later call to apply.
//!
//! A plan opens the file read-only. Apply opens the same path for writing,
//! acquires a write transaction, and recomputes the plan before executing SQL.
//! The migration SQL, migration-history writes, and verification share that
//! transaction. A failed operation rolls back the whole batch, so the caller
//! can retry after fixing the cause. This crate has no down-migration API.
//!
//! Migration SQL and callbacks come from trusted application code. The engine
//! owns the SQLite connection and blocks migration SQL from controlling the
//! transaction, attaching databases, or changing engine metadata. Project
//! callbacks run with read-only SQL permissions. See [`Spec::apply`] for the
//! permission changes during execution.
//!
//! The plan binds the path, schema, and migration history. Row changes can
//! continue after planning. Applications check relevant rows with the
//! callbacks in [`Spec::apply`], which run under the write transaction. A
//! deployment that needs to distinguish otherwise identical database instances
//! should bind its release to a project-specific database identity.

pub mod backup;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use rusqlite::{
    Connection, OpenFlags, TransactionBehavior,
    hooks::{AuthAction, AuthContext, Authorization},
};
use sha2::{Digest, Sha256};

mod rehearsal;
pub use rehearsal::{RehearsalPaths, RehearsalReport};

const MIGRATION_HISTORY_TABLE: &str = "evolve_sqlite_migration";
// This engine-owned table records committed migrations in ordinal order, with
// their frozen SQL checksum and application time. Keep its name and DDL bytes
// stable. The DDL constant is the full CREATE TABLE statement. plan_on compares
// sqlite_schema.sql with this exact stored definition before trusting history,
// including databases created by previous versions.
const MIGRATION_HISTORY_DDL: &str = "CREATE TABLE evolve_sqlite_migration (
                    ordinal INTEGER PRIMARY KEY CHECK (ordinal > 0),
                    name TEXT NOT NULL UNIQUE,
                    sha256 TEXT NOT NULL,
                    applied_at INTEGER NOT NULL,
                    outcome TEXT NOT NULL CHECK (outcome = 'applied')
                ) STRICT";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// An error returned by a project check inside the migration transaction.
pub type CheckError = Box<dyn std::error::Error + Send + Sync>;

type LegacyReader<'a> =
    dyn Fn(&Connection) -> Result<Vec<LegacyMigration>, CheckError> + 'a;

/// A frozen SQL step from the preceding database version to the next one.
///
/// The application embeds every step in order, with contiguous names such as
/// `01_create_records.sql` and `02_add_notes.sql`. Fresh databases run that
/// sequence from the beginning; existing databases run only missing steps.
/// Reproducing a schema from an empty database does not require any individual
/// step to be safe to rerun on an already migrated database.
///
/// A step and its history row commit together. If SQL or a project check fails,
/// the whole apply transaction rolls back and the pending steps can be retried.
/// Freeze the name and SQL bytes once applied, because planning checks them
/// against recorded history. Down migrations are not implemented by this crate.
#[derive(Clone, Copy, Debug)]
pub struct Migration<'a> {
    /// A contiguous two-digit ordinal and lowercase name ending in `.sql`.
    pub name: &'a str,
    /// SQL whose exact UTF-8 bytes determine the recorded SHA-256 checksum.
    pub sql: &'a str,
}

/// The application's migration specification for one database kind.
///
/// Define one spec per database kind, such as records or application state.
/// It binds that kind's identity to its complete, ordered SQL history. Planning
/// and applying both use this description; constructing a spec executes no SQL.
/// The engine stores the applied migration count in SQLite `user_version` and
/// checks it against the migration-history table before planning.
#[derive(Clone, Copy, Debug)]
pub struct Spec<'a> {
    /// A distinct nonzero SQLite `application_id` for this database kind.
    ///
    /// This identifies a file's intended application, not an individual file or
    /// deployment. Copies share the value. Use a project check if instance
    /// identity matters. Empty files receive this value during explicit apply;
    /// existing unmarked databases require explicit legacy adoption.
    pub application_id: i32,
    /// The complete frozen history in contiguous ordinal order, starting at 01.
    pub migrations: &'a [Migration<'a>],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedMigration {
    pub name: String,
    pub sha256: String,
}

/// Project-validated evidence of work already applied before engine adoption.
///
/// A project validator maps legacy SQL migrations or historical project
/// operations to an initial prefix of the spec. It must prove that the schema
/// and original history match that prefix. Adoption records the evidence
/// without executing the mapped SQL again and leaves the original history in
/// place. The mapped checksum must equal the embedded migration's checksum;
/// mapping a filename or ordinal does not permit changing the original SQL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyMigration {
    /// The mapped, one-based ordinal in the spec, not the old history's ordinal.
    pub ordinal: i64,
    /// The exact migration name in the spec.
    pub name: String,
    /// The original SQL checksum, which must match the spec's frozen SQL.
    pub sha256: String,
    /// The original application time in Unix seconds, preserved during adoption.
    pub applied_at: i64,
}

/// The database state and migration list observed during planning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    path: PathBuf,
    application_id: i32,
    state: DatabaseState,
    migrations: Vec<PlannedMigration>,
    legacy: Vec<LegacyMigration>,
}

impl Plan {
    pub fn pending(&self) -> &[PlannedMigration] {
        &self.migrations[self.applied()..]
    }

    pub fn adopted(&self) -> &[LegacyMigration] {
        &self.legacy
    }

    fn applied(&self) -> usize {
        self.state.migration_applied_at.len() + self.legacy.len()
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("another migration holds the lock at {path}")]
    Locked { path: PathBuf },
    #[error(
        "rehearsal failed; snapshot {snapshot_path}, copy {copy_path}: {source}"
    )]
    Rehearsal {
        snapshot_path: PathBuf,
        copy_path: PathBuf,
        source: Box<Error>,
    },
    #[error(
        "verified apply failed; snapshot {snapshot_path}, copy {copy_path}: {source}"
    )]
    VerifiedApply {
        snapshot_path: PathBuf,
        copy_path: PathBuf,
        source: Box<Error>,
    },
    #[error(transparent)]
    Backup(#[from] backup::BackupError),
    #[error("invalid migration spec: {0}")]
    InvalidSpec(String),
    #[error("expected database application_id {expected}, got {actual}")]
    WrongDatabase { expected: i32, actual: i32 },
    #[error(
        "unmarked database contains existing state and requires explicit adoption"
    )]
    AdoptionRequired,
    #[error("expected an unmarked legacy database with an existing schema")]
    NotLegacy,
    #[error("legacy history validation failed: {0}")]
    LegacyValidation(#[source] CheckError),
    #[error("database migration {0} is newer than this spec")]
    Ahead(String),
    #[error("migration ledger drift: {0}")]
    Drift(String),
    #[error("the database or migration spec changed after planning")]
    StalePlan,
    #[error("migration {name} failed: {source}")]
    Migration {
        name: String,
        source: rusqlite::Error,
    },
    #[error("foreign key check failed")]
    ForeignKeyViolation,
    #[error("database integrity check failed: {0}")]
    IntegrityCheck(String),
    #[error("database verification failed: {0}")]
    Verification(#[source] CheckError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Stable identifiers for callers that present structured errors.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Locked { .. } => "migration_locked",
            Self::Backup(source) => source.code(),
            Self::Rehearsal { source, .. } => source.code(),
            Self::VerifiedApply { source, .. } => source.code(),
            Self::InvalidSpec(_) => "invalid_spec",
            Self::WrongDatabase { .. } => "wrong_database",
            Self::AdoptionRequired => "adoption_required",
            Self::NotLegacy => "not_legacy",
            Self::LegacyValidation(_) => "legacy_validation",
            Self::Ahead(_) => "ahead",
            Self::Drift(_) => "drift",
            Self::StalePlan => "stale_plan",
            Self::Migration { .. } => "migration",
            Self::ForeignKeyViolation => "foreign_key_violation",
            Self::IntegrityCheck(_) => "integrity_check_failed",
            Self::Verification(_) => "verification",
            Self::Sqlite(_) => "sqlite",
            Self::Io(_) => "io",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DatabaseState {
    application_id: i32,
    user_version: i32,
    schema: Vec<SchemaObject>,
    // The validated ordinal, name, hash, and outcome are implied by the spec.
    // Keep the remaining column to detect changes to any observed history row.
    migration_applied_at: Vec<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SchemaObject {
    kind: String,
    name: String,
    table: String,
    sql: Option<String>,
}

impl Spec<'_> {
    /// Read the current state of an existing SQLite file without writing to it.
    ///
    /// An empty, unmarked file is eligible for initialization. An unmarked
    /// file with existing schema or a nonzero `user_version` needs a separate
    /// adoption procedure.
    pub fn plan(&self, path: impl AsRef<Path>) -> Result<Plan, Error> {
        self.read_plan(path.as_ref(), None)
    }

    /// Plan explicit adoption of an unmarked legacy database.
    ///
    /// The project callback validates the old schema and original history,
    /// then returns the applied entries mapped to the exact names and checksums
    /// of an initial prefix of this spec. Entries can have different ordinals
    /// in the original history. The callback must check the original evidence,
    /// including its checksums. The engine preserves the old history table and
    /// records mapped entries with their original application times on apply.
    /// Planning executes none of the mapped SQL and changes no history.
    pub fn plan_adoption(
        &self,
        path: impl AsRef<Path>,
        read_legacy: impl Fn(&Connection) -> Result<Vec<LegacyMigration>, CheckError>,
    ) -> Result<Plan, Error> {
        self.read_plan(path.as_ref(), Some(&read_legacy))
    }

    fn read_plan(
        &self,
        path: &Path,
        read_legacy: Option<&LegacyReader<'_>>,
    ) -> Result<Plan, Error> {
        let migrations = self.validate()?;
        let path = path.canonicalize()?;
        let mut conn = open(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let authorizer = SqlAuthorizer::install(&mut conn)?;
        let tx = conn.transaction()?;
        let _restricted =
            authorizer.restrict_to(SqlPermissionMode::ProjectCheck);
        self.plan_on(&tx, path, migrations, read_legacy)
    }

    /// Apply a plan and compare project data within the write transaction.
    ///
    /// The first callback captures a read-only baseline before migration. The
    /// second checks the result before commit. An error from either callback
    /// rolls back the batch. For a rehearsal, compute a plan on the copy.
    /// The caller arranges rehearsal, backup, and retention before applying.
    ///
    /// The SQL authorizer starts with engine permissions to begin the
    /// transaction. It switches to read-only project-check permissions while
    /// revalidating the plan and capturing the baseline. Engine permissions
    /// then allow identity and history initialization. Each pending SQL step
    /// runs with migration permissions, which prohibit transaction control,
    /// attached databases, history-table writes, and table drops. Engine
    /// permissions record each completed step and check foreign keys. The final
    /// callback runs read-only, then engine permissions allow commit. Each
    /// restricted scope restores engine permissions when it exits, including
    /// on error, so the transaction can roll back.
    ///
    /// Applied steps are skipped. After a successful apply, obtain a fresh plan
    /// before another call; the previous plan describes the pre-apply state.
    pub fn apply<T>(
        &self,
        plan: &Plan,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<(), Error> {
        self.apply_plan(plan, None, capture, verify)
    }

    /// Revalidate legacy history and adopt it in the migration transaction.
    ///
    /// The project supplies the same validation it used to plan the adoption.
    /// The engine records the verified legacy prefix, then runs any pending
    /// migrations. It does not rerun SQL for the adopted prefix. Original
    /// checksums and application times are preserved in the mapped history.
    /// Revalidation and project checks run read-only, as in [`Self::apply`].
    /// The verification callback can reject the entire operation, including
    /// identity assignment and history adoption.
    pub fn apply_adoption<T>(
        &self,
        plan: &Plan,
        read_legacy: impl Fn(&Connection) -> Result<Vec<LegacyMigration>, CheckError>,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<(), Error> {
        self.apply_plan(plan, Some(&read_legacy), capture, verify)
    }

    fn apply_plan<T>(
        &self,
        plan: &Plan,
        read_legacy: Option<&LegacyReader<'_>>,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<(), Error> {
        self.apply_with_preparation(
            plan,
            read_legacy,
            || Ok(()),
            capture,
            verify,
        )
    }

    fn apply_with_preparation<T, R>(
        &self,
        plan: &Plan,
        read_legacy: Option<&LegacyReader<'_>>,
        prepare: impl FnOnce() -> Result<R, Error>,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<R, Error> {
        let _lock = rehearsal::MigrationLock::acquire(&plan.path)?;
        let migrations = self.validate()?;
        let mut conn = open(&plan.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let authorizer = SqlAuthorizer::install(&mut conn)?;
        let tx =
            conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let baseline = {
            let _restricted =
                authorizer.restrict_to(SqlPermissionMode::ProjectCheck);
            let current =
                self.plan_on(&tx, plan.path.clone(), migrations, read_legacy)?;
            if &current != plan {
                return Err(Error::StalePlan);
            }
            capture(&tx).map_err(Error::Verification)?
        };
        let preparation = prepare()?;

        if plan.state.application_id == 0 {
            tx.pragma_update(None, "application_id", self.application_id)?;
            tx.execute_batch(MIGRATION_HISTORY_DDL)?;
            for migration in &plan.legacy {
                tx.execute(
                    "INSERT INTO evolve_sqlite_migration (ordinal, name, sha256, applied_at, outcome)
                     VALUES (?1, ?2, ?3, ?4, 'applied')",
                    (migration.ordinal, &migration.name, &migration.sha256, migration.applied_at),
                )?;
            }
        }

        for (index, migration) in
            self.migrations.iter().enumerate().skip(plan.applied())
        {
            {
                let _restricted =
                    authorizer.restrict_to(SqlPermissionMode::Migration);
                tx.execute_batch(migration.sql).map_err(|source| {
                    Error::Migration {
                        name: migration.name.to_owned(),
                        source,
                    }
                })?;
            }
            tx.execute(
                "INSERT INTO evolve_sqlite_migration (ordinal, name, sha256, applied_at, outcome)
                 VALUES (?1, ?2, ?3, unixepoch(), 'applied')",
                ((index + 1) as i64, migration.name, &plan.migrations[index].sha256),
            )?;
        }

        if !plan.pending().is_empty() || !plan.legacy.is_empty() {
            tx.pragma_update(
                None,
                "user_version",
                plan.migrations.len() as i32,
            )?;
        }
        let foreign_key_violation =
            tx.prepare("PRAGMA foreign_key_check")?.exists([])?;
        if foreign_key_violation {
            return Err(Error::ForeignKeyViolation);
        }
        let integrity: String =
            tx.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(Error::IntegrityCheck(integrity));
        }
        {
            let _restricted =
                authorizer.restrict_to(SqlPermissionMode::ProjectCheck);
            verify(&tx, baseline).map_err(Error::Verification)?;
        }
        tx.commit()?;
        Ok(preparation)
    }

    fn validate(&self) -> Result<Vec<PlannedMigration>, Error> {
        if self.application_id == 0 {
            return Err(Error::InvalidSpec(
                "application_id must be nonzero".into(),
            ));
        }
        self.migrations
            .iter()
            .enumerate()
            .map(|(index, migration)| {
                let ordinal = index + 1;
                let valid_name = migration
                    .name
                    .strip_prefix(&format!("{ordinal:02}_"))
                    .and_then(|name| name.strip_suffix(".sql"))
                    .is_some_and(|name| {
                        name.split('_').all(|part| {
                            !part.is_empty()
                                && part.bytes().all(|b| {
                                    b.is_ascii_lowercase() || b.is_ascii_digit()
                                })
                        })
                    });
                if ordinal > 99 || !valid_name || migration.sql.contains('\0') {
                    return Err(Error::InvalidSpec(format!(
                        "invalid migration {ordinal}: {:?}",
                        migration.name
                    )));
                }
                Ok(PlannedMigration {
                    name: migration.name.to_owned(),
                    sha256: format!(
                        "{:x}",
                        Sha256::digest(migration.sql.as_bytes())
                    ),
                })
            })
            .collect()
    }

    fn plan_on(
        &self,
        conn: &Connection,
        path: PathBuf,
        migrations: Vec<PlannedMigration>,
        read_legacy: Option<&LegacyReader<'_>>,
    ) -> Result<Plan, Error> {
        let application_id: i32 =
            conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        if application_id != 0 && application_id != self.application_id {
            return Err(Error::WrongDatabase {
                expected: self.application_id,
                actual: application_id,
            });
        }
        let user_version =
            conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let schema = conn
            .prepare("SELECT type, name, tbl_name, sql FROM main.sqlite_schema ORDER BY type, name")?
            .query_map([], |row| {
                Ok(SchemaObject {
                    kind: row.get(0)?,
                    name: row.get(1)?,
                    table: row.get(2)?,
                    sql: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let legacy = if let Some(read_legacy) = read_legacy {
            if application_id != 0 || schema.is_empty() {
                return Err(Error::NotLegacy);
            }
            if schema.iter().any(|object| {
                object.name.eq_ignore_ascii_case(MIGRATION_HISTORY_TABLE)
            }) {
                return Err(Error::Drift(format!(
                    "legacy schema already contains {MIGRATION_HISTORY_TABLE}"
                )));
            }
            let legacy = read_legacy(conn).map_err(Error::LegacyValidation)?;
            validate_legacy(&legacy, &migrations)?;
            legacy
        } else {
            Vec::new()
        };
        let migration_applied_at = if application_id == 0 {
            if legacy.is_empty() && (user_version != 0 || !schema.is_empty()) {
                return Err(Error::AdoptionRequired);
            }
            Vec::new()
        } else {
            let ledger = schema
                .iter()
                .find(|object| {
                    object.kind == "table"
                        && object.name == MIGRATION_HISTORY_TABLE
                })
                .ok_or_else(|| {
                    Error::Drift(format!(
                        "missing table {MIGRATION_HISTORY_TABLE}"
                    ))
                })?;
            if ledger.sql.as_deref() != Some(MIGRATION_HISTORY_DDL) {
                return Err(Error::Drift(format!(
                    "unexpected schema for {MIGRATION_HISTORY_TABLE}"
                )));
            }
            if let Some(trigger) = schema.iter().find(|object| {
                object.kind == "trigger"
                    && object.table.eq_ignore_ascii_case(MIGRATION_HISTORY_TABLE)
            }) {
                return Err(Error::Drift(format!(
                    "unexpected trigger {} on {MIGRATION_HISTORY_TABLE}",
                    trigger.name
                )));
            }
            let applied_at = validate_history(conn, &migrations)?;
            let applied = applied_at.len();
            if user_version != applied as i32 {
                return Err(Error::Drift(format!(
                    "user_version {user_version} disagrees with ledger length {applied}"
                )));
            }
            applied_at
        };
        Ok(Plan {
            path,
            application_id: self.application_id,
            state: DatabaseState {
                application_id,
                user_version,
                schema,
                migration_applied_at,
            },
            migrations,
            legacy,
        })
    }
}

fn open(path: &Path, mode: OpenFlags) -> Result<Connection, Error> {
    let conn = Connection::open_with_flags(
        path,
        mode | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

fn validate_legacy(
    legacy: &[LegacyMigration],
    expected: &[PlannedMigration],
) -> Result<(), Error> {
    if legacy.is_empty() || legacy.len() > expected.len() {
        return Err(Error::Drift(
            "legacy history must map to a nonempty migration prefix".into(),
        ));
    }
    for (index, (legacy, migration)) in legacy.iter().zip(expected).enumerate() {
        if legacy.ordinal != index as i64 + 1
            || legacy.name != migration.name
            || legacy.sha256 != migration.sha256
        {
            return Err(Error::Drift(format!(
                "legacy history entry {:?} does not match migration {}",
                legacy.name, migration.name,
            )));
        }
    }
    Ok(())
}

fn validate_history(
    conn: &Connection,
    expected: &[PlannedMigration],
) -> Result<Vec<i64>, Error> {
    let mut statement = conn.prepare(
        "SELECT ordinal, name, sha256, outcome, applied_at
         FROM main.evolve_sqlite_migration ORDER BY ordinal",
    )?;
    let mut rows = statement.query([])?;
    let mut applied_at = Vec::new();
    while let Some(row) = rows.next()? {
        let index = applied_at.len();
        let ordinal: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        if ordinal != index as i64 + 1 {
            return Err(Error::Drift(format!(
                "unexpected ordinal {ordinal} for {name}"
            )));
        }
        let Some(migration) = expected.get(index) else {
            return Err(Error::Ahead(name));
        };
        let checksum: String = row.get(2)?;
        let outcome: String = row.get(3)?;
        if name != migration.name
            || checksum != migration.sha256
            || outcome != "applied"
        {
            return Err(Error::Drift(format!(
                "migration {name} does not match the spec"
            )));
        }
        applied_at.push(row.get(4)?);
    }
    Ok(applied_at)
}

/// SQL permissions on the engine-owned connection, not migration progress.
///
/// The authorizer begins in Engine mode. A guard temporarily selects Migration
/// or ProjectCheck mode and restores Engine when dropped. Restricted scopes
/// are not nested. The surrounding transaction spans all these mode changes.
#[repr(u8)]
enum SqlPermissionMode {
    /// Allow engine statements, including transaction and metadata writes.
    Engine,
    /// Allow project SQL within main, subject to migration_action restrictions.
    Migration,
    /// Allow reads and approved inspection pragmas for planning and callbacks.
    ProjectCheck,
}

/// Installs SQLite's statement authorizer and selects its SQL permission mode.
struct SqlAuthorizer(Arc<AtomicU8>);

impl SqlAuthorizer {
    fn install(conn: &mut Connection) -> Result<Self, Error> {
        let mode = Arc::new(AtomicU8::new(SqlPermissionMode::Engine as u8));
        let callback_mode = Arc::clone(&mode);
        conn.authorizer(Some(move |ctx: AuthContext<'_>| {
            let mode = callback_mode.load(Ordering::Relaxed);
            let allowed = if mode == SqlPermissionMode::Migration as u8 {
                migration_action(ctx)
            } else if mode == SqlPermissionMode::ProjectCheck as u8 {
                verification_action(ctx.action)
            } else {
                true
            };
            if allowed {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }))?;
        Ok(Self(mode))
    }

    fn restrict_to(&self, mode: SqlPermissionMode) -> SqlPermissionGuard<'_> {
        self.0.store(mode as u8, Ordering::Relaxed);
        SqlPermissionGuard(self)
    }
}

/// Restores engine permissions when a restricted SQL scope exits.
struct SqlPermissionGuard<'a>(&'a SqlAuthorizer);

impl Drop for SqlPermissionGuard<'_> {
    fn drop(&mut self) {
        self.0
            .0
            .store(SqlPermissionMode::Engine as u8, Ordering::Relaxed);
    }
}

fn migration_action(ctx: AuthContext<'_>) -> bool {
    if ctx.database_name.is_some_and(|name| name != "main") {
        return false;
    }
    match ctx.action {
        AuthAction::Unknown { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        // A DROP can implicitly cascade-delete dependent rows with foreign keys on.
        | AuthAction::DropTable { .. }
        | AuthAction::DropVtable { .. } => false,
        AuthAction::Pragma {
            pragma_name,
            pragma_value,
        } => {
            // FTS5 reads these while creating or opening its shadow tables.
            pragma_value.is_none()
                && pragma_is(pragma_name, &["data_version", "page_size"])
        }
        AuthAction::Delete { table_name }
        | AuthAction::Insert { table_name }
        | AuthAction::Update { table_name, .. }
        | AuthAction::AlterTable { table_name, .. }
        | AuthAction::CreateIndex { table_name, .. }
        | AuthAction::CreateTrigger { table_name, .. } => {
            !table_name.eq_ignore_ascii_case(MIGRATION_HISTORY_TABLE)
        }
        AuthAction::Function { function_name } => {
            !function_name.eq_ignore_ascii_case("load_extension")
        }
        _ => true,
    }
}

fn verification_action(action: AuthAction<'_>) -> bool {
    match action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Recursive => {
            true
        }
        AuthAction::Function { function_name } => {
            !function_name.eq_ignore_ascii_case("load_extension")
        }
        AuthAction::Pragma {
            pragma_name,
            pragma_value,
        } => {
            pragma_is(
                pragma_name,
                &[
                    "foreign_key_check",
                    "integrity_check",
                    "quick_check",
                    "table_info",
                    "table_xinfo",
                    "index_list",
                    "index_info",
                    "index_xinfo",
                    "foreign_key_list",
                ],
            ) || (pragma_value.is_none()
                && pragma_is(
                    pragma_name,
                    &[
                        "data_version",
                        "page_size",
                        "application_id",
                        "user_version",
                    ],
                ))
        }
        _ => false,
    }
}

fn pragma_is(name: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|entry| name.eq_ignore_ascii_case(entry))
}
