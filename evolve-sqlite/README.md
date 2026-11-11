# evolve-sqlite

A Rust library for explicit SQLite migration planning and execution. The
first slice runs against disposable databases and copies. It covers
initialization, transactional updates, and adoption of verified legacy
migration history.

## Migrations and History

Each `Migration` is a frozen SQL step from one database version to the next.
Its filename determines its position in a contiguous sequence, starting at
`01_name.sql`. The engine records each applied name, SQL checksum, and Unix
application time in table `evolve_sqlite_migration`. Its `ordinal` column
records the order, and SQLite `user_version` records the applied count.

A fresh database runs the whole sequence. An existing database runs only
the pending suffix after its recorded history. This lets both reach the
schema described by the same SQL history. Individual files need not be
idempotent. For example, a plain `CREATE TABLE` can run once without an
`IF NOT EXISTS` clause.

A failed apply rolls back all SQL and history writes in its transaction.
After fixing the cause, the caller can retry the uncommitted batch. After
a successful apply, a fresh plan skips the committed steps. Retrying a
failed transaction and replaying already applied SQL are different
operations. This crate does not implement down migrations or automatic
rollback of committed changes.

## Plan and Apply

1. Define a `Spec`, the migration specification for one database kind,
   with a nonzero SQLite `application_id` and embedded SQL
   files named `01_name.sql`, `02_name.sql`, and so on. Freeze each file's
   bytes once it has been applied.
2. Call `Spec::plan` on an existing SQLite file. Review the pending names
   and checksums. To initialize a database, use an empty file.
3. Call `Spec::apply` with the reviewed plan and two project checks. The
   first captures a baseline and the second compares it with the migrated
   state. The engine checks the plan again under a write transaction.
   Migration SQL, history changes, and the project checks share that
   transaction.

The `Spec` struct holds the database kind's identity and complete migration
sequence. Constructing it executes no SQL. Choose a distinct `application_id`
for each database kind, such as records or application state. This SQLite
pragma identifies the intended application, not an individual database
instance. Copies retain the same value. An explicit apply marks an empty
file; an existing unmarked database requires legacy adoption.

The plan records the canonical path, schema, and migration history. It
allows application rows to change after planning. A deployment that needs
to distinguish otherwise identical databases at the same path should
validate its database instance identity in a project check.

## SQL Permissions During Apply

The engine owns the connection and installs a SQLite authorizer. Its private
`SqlPermissionMode` enum selects which SQL actions that authorizer permits.
These modes describe permissions, not schema versions or migration progress.

1. Engine permissions allow the immediate write transaction to begin.
2. Read-only project-check permissions cover plan revalidation and the
   baseline callback. They also cover the legacy validator during adoption.
3. Engine permissions allow identity assignment and history-table creation.
4. Migration permissions cover each pending SQL file. They prohibit
   transaction control, attached databases, history-table writes, and table
   drops. After each file, engine permissions allow its history insert.
5. Engine permissions allow the foreign-key check. Read-only project-check
   permissions cover the final verification callback, then engine permissions
   allow commit.

Each restricted scope restores engine permissions when it exits, including
on failure. The transaction can then roll back the batch. Planning uses the
same read-only permissions on a read-only connection. Migration SQL and
callbacks still come from trusted application code; these permissions
protect the engine's transaction and metadata.

## Adopt a Legacy Database

Use `Spec::plan_adoption` and `Spec::apply_adoption` with the same
project-specific validator. The validator checks the old schema and
history and returns `LegacyMigration` entries mapped to the spec's initial
migration sequence. Each entry is evidence of SQL or a project operation
that already ran. The validator must prove that the old database corresponds
to the mapped SQL prefix, including checking the original history and its
checksums. The engine checks the mapped ordinals, names, and checksums
against the spec.

Adoption preserves the original SQL checksums and application timestamps.
It records the mapped prefix without executing that SQL again, then runs
any pending migrations. The engine leaves the original history table in
place, where project-specific evidence remains available. It sets SQLite
`user_version` to the length of the engine's migration history. A failure
rolls back the adoption and pending SQL together.

## Test the Slice

From the crate directory, run `cargo test --locked`. The tests create
temporary databases, including an unmarked legacy database with a
different numbering scheme. They check history and row preservation,
checksum mismatch, stale plans, and rollback after failed SQL or a
failed project check.

The caller arranges backup and rehearsal for this slice. The engine
rejects `DROP TABLE` while foreign-key enforcement is on because a table
rebuild can cascade-delete dependent rows. Production integration,
engine-managed rehearsal, table rebuild support, repeatable SQL, and
down migrations have their own follow-up work.
