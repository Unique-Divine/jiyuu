//! Plan and apply frozen SQL migrations against a known SQLite database.
//!
//! A plan opens the file read-only. Apply opens the same path for writing,
//! acquires a write transaction, and recomputes the plan before executing SQL.
//! The migration SQL, ledger writes, and verification share that transaction.
//!
//! Migration SQL and callbacks come from trusted application code. The engine
//! owns the SQLite connection and blocks migration SQL from controlling the
//! transaction, attaching databases, or changing engine metadata.
//!
//! The plan binds the path, schema, and migration history. Row changes can
//! continue after planning. Applications check relevant rows with the
//! callbacks in [`Spec::apply`], which run under the write transaction.

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

const LEDGER: &str = "evolve_sqlite_migration";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// An error returned by a project check inside the migration transaction.
pub type CheckError = Box<dyn std::error::Error + Send + Sync>;

/// A frozen migration embedded by the application.
#[derive(Clone, Copy, Debug)]
pub struct Migration<'a> {
    pub name: &'a str,
    pub sql: &'a str,
}

/// An ordered set of migrations for one kind of database.
///
/// Choose a distinct nonzero SQLite `application_id` for each database kind.
/// The engine stores the applied migration count in SQLite `user_version` and
/// checks it against the ledger before planning.
#[derive(Clone, Copy, Debug)]
pub struct Spec<'a> {
    pub application_id: i32,
    pub migrations: &'a [Migration<'a>],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedMigration {
    pub name: String,
    pub sha256: String,
}

/// The database state and migration list observed during planning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    path: PathBuf,
    application_id: i32,
    state: DatabaseState,
    migrations: Vec<PlannedMigration>,
    applied: usize,
}

impl Plan {
    pub fn pending(&self) -> &[PlannedMigration] {
        &self.migrations[self.applied..]
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid migration spec: {0}")]
    InvalidSpec(String),
    #[error("expected database application_id {expected}, got {actual}")]
    WrongDatabase { expected: i32, actual: i32 },
    #[error(
        "unmarked database contains existing state and requires explicit adoption"
    )]
    AdoptionRequired,
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
    #[error("database verification failed: {0}")]
    Verification(#[source] CheckError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DatabaseState {
    application_id: i32,
    user_version: i32,
    schema: Vec<SchemaObject>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SchemaObject {
    kind: String,
    name: String,
    sql: Option<String>,
}

impl Spec<'_> {
    /// Read the current state of an existing SQLite file without writing to it.
    ///
    /// An empty, unmarked file is eligible for initialization. An unmarked
    /// file with existing schema or a nonzero `user_version` needs a separate
    /// adoption procedure.
    pub fn plan(&self, path: impl AsRef<Path>) -> Result<Plan, Error> {
        let migrations = self.validate()?;
        let path = path.as_ref().canonicalize()?;
        let mut conn = open(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let tx = conn.transaction()?;
        self.plan_on(&tx, path, migrations)
    }

    /// Apply a plan and compare project data within the write transaction.
    ///
    /// The first callback captures a read-only baseline before migration. The
    /// second checks the result before commit. An error from either callback
    /// rolls back the batch. For a rehearsal, compute a plan on the copy.
    /// The caller arranges rehearsal, backup, and retention before applying.
    pub fn apply<T>(
        &self,
        plan: &Plan,
        capture: impl FnOnce(&Connection) -> Result<T, CheckError>,
        verify: impl FnOnce(&Connection, T) -> Result<(), CheckError>,
    ) -> Result<(), Error> {
        let migrations = self.validate()?;
        let mut conn = open(&plan.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let authorizer = Authorizer::install(&mut conn)?;
        let tx =
            conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = self.plan_on(&tx, plan.path.clone(), migrations)?;
        if &current != plan {
            return Err(Error::StalePlan);
        }

        let baseline = {
            let _restricted = authorizer.restrict(Phase::Verification);
            capture(&tx).map_err(Error::Verification)?
        };

        if plan.state.application_id == 0 {
            tx.pragma_update(None, "application_id", self.application_id)?;
            tx.execute_batch(
                "CREATE TABLE evolve_sqlite_migration (
                    ordinal INTEGER PRIMARY KEY CHECK (ordinal > 0),
                    name TEXT NOT NULL UNIQUE,
                    sha256 TEXT NOT NULL,
                    applied_at INTEGER NOT NULL,
                    outcome TEXT NOT NULL CHECK (outcome = 'applied')
                ) STRICT",
            )?;
        }

        for (index, migration) in
            self.migrations.iter().enumerate().skip(plan.applied)
        {
            {
                let _restricted = authorizer.restrict(Phase::Migration);
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

        if !plan.pending().is_empty() {
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
        {
            let _restricted = authorizer.restrict(Phase::Verification);
            verify(&tx, baseline).map_err(Error::Verification)?;
        }
        tx.commit()?;
        Ok(())
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
            .prepare("SELECT type, name, sql FROM main.sqlite_schema ORDER BY type, name")?
            .query_map([], |row| {
                Ok(SchemaObject {
                    kind: row.get(0)?,
                    name: row.get(1)?,
                    sql: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let applied = if application_id == 0 {
            if user_version != 0 || !schema.is_empty() {
                return Err(Error::AdoptionRequired);
            }
            0
        } else {
            if !schema
                .iter()
                .any(|object| object.kind == "table" && object.name == LEDGER)
            {
                return Err(Error::Drift(format!("missing table {LEDGER}")));
            }
            let applied = validate_history(conn, &migrations)?;
            if user_version != applied as i32 {
                return Err(Error::Drift(format!(
                    "user_version {user_version} disagrees with ledger length {applied}"
                )));
            }
            applied
        };
        Ok(Plan {
            path,
            application_id: self.application_id,
            state: DatabaseState {
                application_id,
                user_version,
                schema,
            },
            migrations,
            applied,
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

fn validate_history(
    conn: &Connection,
    expected: &[PlannedMigration],
) -> Result<usize, Error> {
    let mut statement = conn.prepare(
        "SELECT ordinal, name, sha256, outcome FROM main.evolve_sqlite_migration ORDER BY ordinal",
    )?;
    let mut rows = statement.query([])?;
    let mut index = 0;
    while let Some(row) = rows.next()? {
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
        index += 1;
    }
    Ok(index)
}

#[repr(u8)]
enum Phase {
    Engine,
    Migration,
    Verification,
}

struct Authorizer(Arc<AtomicU8>);

impl Authorizer {
    fn install(conn: &mut Connection) -> Result<Self, Error> {
        let phase = Arc::new(AtomicU8::new(Phase::Engine as u8));
        let callback_phase = Arc::clone(&phase);
        conn.authorizer(Some(move |ctx: AuthContext<'_>| {
            let phase = callback_phase.load(Ordering::Relaxed);
            let allowed = if phase == Phase::Migration as u8 {
                migration_action(ctx)
            } else if phase == Phase::Verification as u8 {
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
        Ok(Self(phase))
    }

    fn restrict(&self, phase: Phase) -> Restricted<'_> {
        self.0.store(phase as u8, Ordering::Relaxed);
        Restricted(self)
    }
}

struct Restricted<'a>(&'a Authorizer);

impl Drop for Restricted<'_> {
    fn drop(&mut self) {
        self.0.0.store(Phase::Engine as u8, Ordering::Relaxed);
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
        | AuthAction::Detach { .. } => false,
        AuthAction::Pragma {
            pragma_name,
            pragma_value,
        } => {
            // FTS5 reads these while creating or opening its shadow tables.
            pragma_value.is_none()
                && ["data_version", "page_size"].contains(&pragma_name)
        }
        AuthAction::Delete { table_name }
        | AuthAction::Insert { table_name }
        | AuthAction::Update { table_name, .. }
        | AuthAction::DropTable { table_name }
        | AuthAction::AlterTable { table_name, .. }
        | AuthAction::CreateIndex { table_name, .. }
        | AuthAction::CreateTrigger { table_name, .. } => {
            !table_name.eq_ignore_ascii_case(LEDGER)
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
        } => match pragma_name {
            "foreign_key_check" | "integrity_check" | "quick_check"
            | "table_info" | "table_xinfo" | "index_list" | "index_info"
            | "index_xinfo" | "foreign_key_list" => true,
            "data_version" | "page_size" | "application_id" | "user_version" => {
                pragma_value.is_none()
            }
            _ => false,
        },
        _ => false,
    }
}
