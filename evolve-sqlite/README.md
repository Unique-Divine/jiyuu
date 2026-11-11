# evolve-sqlite

A Rust library for explicit SQLite migration planning and execution. The
engine runs against disposable databases and copies. It covers
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
   with a nonzero `app_id` field and embedded SQL
   files named `01_name.sql`, `02_name.sql`, and so on. Freeze each file's
   bytes once it has been applied.
2. Call `Spec::plan` on an existing SQLite file. Review the pending names
   and checksums. To initialize a database, use an empty file.
3. Call `Spec::apply_verified` with the reviewed plan, unused backup and
   copy destinations, and two project checks. The first captures a baseline
   and the second compares it with the migrated state. The engine checks
   the plan again under a write transaction, creates a verified snapshot,
   and rehearses on a copy before executing the live SQL. Migration SQL,
   history changes, and the project checks share the live transaction.

The `Spec` struct holds the database kind's identity and complete migration
sequence. Constructing it executes no SQL. Choose a distinct `app_id`
for each database kind, such as records or application state. The field maps
to SQLite pragma `application_id`, which identifies the intended application,
not an individual database instance. Copies retain the same value. An explicit apply marks an empty
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
   transaction control, attached databases, and history-table writes. Table
   drops require a rebuild policy in the plan. After each file, engine
   permissions allow its history insert.
5. Engine permissions allow the foreign-key check. Read-only project-check
   permissions cover the final verification callback, then engine permissions
   allow commit.

Each restricted scope restores engine permissions when it exits, including
on failure. The transaction can then roll back the batch. Planning uses the
same read-only permissions on a read-only connection. Migration SQL and
callbacks still come from trusted application code; these permissions
protect the engine's transaction and metadata.

## Adopt a Legacy Database

Use `Spec::plan_adoption` and `Spec::apply_adoption_verified` with the same
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
rolls back the adoption and pending SQL together. The legacy validator proves
history before pending SQL runs. The final project check must also compare the
original history table with its captured baseline. The engine protects its
own history from SQL writes; legacy table names belong to the application.

## Create a Backup

Call function `backup::create(source, destination)` to publish a verified,
standalone SQLite copy. SQLite's backup API includes committed WAL pages while
other connections remain open. The returned `Snapshot` struct records the
canonical path, byte length, and SHA-256 digest of the entire file.

The source opens read-only. The copy must pass SQLite `integrity_check` and
`foreign_key_check`, and use DELETE journal mode before publication. A file,
directory, or symlink at the destination or its `-wal`, `-shm`, or `-journal`
path prevents publication. The caller controls the destination directory and
chooses how long to retain backups.

The `BackupError` enum distinguishes copy and verification failures from
publication durability failures. Its `code()` method returns stable category
identifiers. Failures before publication leave the destination absent and
remove the temporary file. Variant `BackupError::Durability` means the verified
file exists, but its directory sync failed, so its survival across a crash is
uncertain. The variant contains the published path. Missing source or parent
paths return `BackupError::Io` with the filesystem error.

## Rehearse and Apply

Call method `Spec::rehearse` with the reviewed plan, a `RehearsalPaths` struct, and the
project checks. The engine keeps an unchanged snapshot and migrates a
second copy. `Spec::rehearse_adoption` also reruns the legacy validator.
All successful artifacts remain available if a later step fails. Choose
fresh destination paths when retrying. The application owns retention.

Methods `Spec::apply_verified` and `Spec::apply_adoption_verified` hold the
source write reservation throughout snapshot, rehearsal, and live execution.
This keeps the snapshot at the last committed state before migration and excludes application writers
until commit or rollback. Both project checks must support repeated calls.
The live baseline is captured before the snapshot, the copy gets its own
baseline, and each verifier checks its corresponding baseline. Legacy
validation runs separately on source and copy. Methods `Spec::apply` and
`Spec::apply_adoption` remain available for executing prepared copies.

Apply and rehearsal hold an exclusive advisory lock beside the canonical
database path. The lock file stays in place after release so concurrent
processes use the same lock. SQLite transactions exclude other writers
during migration. Participating migrators must use the same canonical
path. The engine checks foreign keys, database integrity, and the project
verifier before committing. A project that requires exact schema matching
supplies that check in its verifier.

## Rebuild a Table

A table rebuild copies rows to a replacement table, drops the original, and
renames the replacement. With foreign-key enforcement enabled, SQLite can
cascade-delete child rows during the drop, even when the replacement has the
same keys. Ordinary methods `Spec::plan` and `Spec::plan_adoption` therefore
continue to reject table drops.

Use struct `RebuildPolicy` to approve specific ordinary tables for a reviewed
rebuild. Method `Spec::plan_with_rebuilds` binds that allowlist into the plan.
For a legacy database, use method `Spec::plan_adoption_with_rebuilds` with the
same legacy validator used for adoption. Inspect method `Plan::rebuilds` when
printing the release plan. All apply and rehearsal methods retain the policy.

```rust,ignore
let rebuilds = RebuildPolicy::new(&["booking"])?;
let plan = spec.plan_with_rebuilds(database_path, &rebuilds)?;
spec.apply_verified(&plan, &paths, capture_bookings, verify_bookings)?;
```

```text
reviewed targets -> FK OFF before BEGIN IMMEDIATE -> copy/drop/rename
                 -> foreign_key_check + integrity_check + project checks
                 -> COMMIT, or roll back SQL and migration history together
```

Migration SQL may set `legacy_alter_table` to preserve existing references
while following a historical rebuild recipe. The engine resets that setting
after each SQL file. SQL cannot change `foreign_keys`, control transactions,
drop virtual tables, or write engine history. SQLite's rename implementation
may inspect and update temporary schema metadata, but project SQL cannot
create temporary objects or access temporary data.

Rebuild permission does not prove data preservation. Capture the application
rows that must survive and compare them before commit, including user-edited
configuration or descriptions. A valid database can still be missing rows.
The engine restores connection settings after the transaction on every exit
and closes the owned connection. Down migrations require separate work.

Error variants `Error::Rehearsal` and `Error::VerifiedApply` retain the
requested artifact paths and nested cause. A requested path may not exist if
an earlier step failed. Their `code()` results identify the underlying failure,
such as `stale_plan` or `verification`; inspect the variant to identify the
operation. Published files remain available for inspection and recovery.

## Verify the Application Schema

Construct struct `schema::Schema` from a trusted, checked-in schema file with
method `Schema::from_sql`. The helper executes that reference in an isolated
in-memory database and compares SQLite's stored schema. Do not generate the
reference from the live database you intend to check.

```rust,ignore
let expected = Schema::from_sql(include_str!("schema.sql"))?;
spec.apply_verified(&plan, &paths, capture_rows, |conn, before| {
    expected.verify(conn)?;
    verify_preserved_rows(conn, before)?;
    Ok(())
})?;
```

Use the same reference for fresh installations, upgraded databases, rehearsal
copies, and live verification. The helper compares the complete table, index,
view, and trigger inventory, including FTS shadow schema generated from virtual
table declarations. Engine history has its own immutable DDL check and is
excluded from the application inventory. Other extra objects cause a mismatch.
For adopted databases, append the exact expected legacy-history DDL explicitly.
Compare its original rows separately in the project verifier.

The comparator ignores comments, whitespace between tokens, and ASCII case in
unquoted words. It preserves literal bytes, quoted identifier bytes, and token
boundaries. It accepts formatting differences without erasing a changed default
or CHECK constraint. It may reject two differently written but equivalent
schemas. A mismatch rejects the transaction; the helper does not repair drift.
Row preservation, legacy-history evidence, foreign keys, and integrity remain
separate checks.

## Replace Views and FTS Maintenance Triggers

Declare replaceable derived definitions with struct `DerivedSql`. Unlike frozen
versioned SQL, these definitions rerun when their checksum changes. They may
replace views and triggers. They cannot change application rows, table
structure, engine metadata, or transaction boundaries. Business seeds remain
explicit create-once or versioned operations so edited defaults survive.

Use method `Spec::plan_with_derived`, or method
`Spec::plan_adoption_with_derived` for a legacy database. Both take an explicit
`RebuildPolicy` for versioned SQL, which may be empty. Method
`Plan::pending_derived` lists changed definitions and their FTS rebuild targets.
The plan owns their SQL and permissions through rehearsal and apply.

```rust,ignore
let derived = [DerivedSql {
    name: "note_search",
    sql: include_str!("derived/note_search.sql"),
    fts_rebuilds: &["note_fts"],
}];
let plan = spec.plan_with_derived(database_path, &RebuildPolicy::default(), &derived)?;
```

```text
pending versioned SQL -> changed derived view/trigger DDL
                     -> rebuild each declared existing FTS5 table
                     -> checksum records -> schema/row checks -> COMMIT
```

Use guarded DROP/CREATE statements in derived SQL. FTS table creation and
structural changes belong to versioned migrations. When maintenance triggers
or an FTS content view change, declare their existing FTS5 table in field
`fts_rebuilds`. The engine applies all pending view/trigger definitions first,
then rebuilds each declared index once from its final source content. History
updates and checks follow in the same transaction. The SQL checksum includes
the ordered FTS target list.

Rebuild writes are restricted to each declared virtual table and its actual
shadows. SQLite can label an ordinary suffix lookalike as `shadow` even when an
external-content or `columnsize=0` index does not create that table. The engine
therefore materializes only the validated, single-statement FTS5 constructor in
a restricted private in-memory database and intersects its created shadows with
the main database's inventory. No source rows or triggers are copied. Derived
maintenance triggers must attach to ordinary source tables. Triggers on index
storage are rejected before any rebuild.

Table `evolve_sqlite_derived` stores the current name, checksum, and application
time separately from frozen migration history. Unchanged definitions keep their
timestamps and execute no SQL. A source, definition, rebuild, or final-check
failure rolls back the entire batch. Existing names cannot disappear from the
supplied declarations. To remove an object, retain its stable declaration name
and change its SQL to an explicit DROP. Callers without those declarations
refuse a database with recorded derived definitions.

Derived SQL does not repair unreported drift automatically. Verify the complete
expected schema and source rows before committing. Original legacy history
still requires its own preservation check.

## Check the Crate

From the crate directory, run command `just check` for formatting, Clippy, and
tests. Commands `just test`, `just fmt-check`, and `just lint` run each check
separately. From the repository root, command `just rust-test` runs all Rust
workspace tests with the committed dependency resolution.

Migration tests cover history and row preservation, stale plans, legacy
adoption, and transaction rollback. Backup tests cover live WAL data,
verification failures, occupied destination paths, concurrent publication,
and failures before and after publication.
