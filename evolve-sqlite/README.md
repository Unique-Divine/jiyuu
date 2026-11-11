# evolve-sqlite

A Rust library for explicit SQLite migration planning and execution. The
first slice runs against disposable databases and copies. It covers
initialization, transactional updates, and adoption of verified legacy
migration history.

## Plan and Apply

1. Define a `Spec` with a nonzero SQLite `application_id` and embedded SQL
   files named `01_name.sql`, `02_name.sql`, and so on. Freeze each file's
   bytes once it has been applied.
2. Call `Spec::plan` on an existing SQLite file. Review the pending names
   and checksums. To initialize a database, use an empty file.
3. Call `Spec::apply` with the reviewed plan and two project checks. The
   first captures a baseline and the second compares it with the migrated
   state. The engine checks the plan again under a write transaction.
   Migration SQL, ledger changes, and the project checks share that
   transaction.

The plan records the canonical path, schema, and migration history. It
allows application rows to change after planning. A deployment that needs
to distinguish otherwise identical databases at the same path should
validate its database instance identity in a project check.

## Adopt a Legacy Database

Use `Spec::plan_adoption` and `Spec::apply_adoption` with the same
project-specific validator. The validator checks the old schema and
history and maps the applied entries to the new spec's initial migration
sequence. Adoption records that sequence and its original application
times, then executes pending migrations. The engine leaves the original
ledger in place. It uses SQLite `user_version` to record the length of
the new ledger.

## Test the Slice

From the crate directory, run `cargo test --locked`. The tests create
temporary databases, including an unmarked legacy database with a
different numbering scheme. They check history and row preservation,
checksum mismatch, stale plans, and rollback after failed SQL or a
failed project check.

## Back Up and Rehearse

Call `backup::create` with a source database and an unused destination to
publish a verified SQLite snapshot, including committed rows in its WAL.
The returned metadata includes the path, size, and SHA-256 digest.

Call `Spec::rehearse` with the reviewed plan, `RehearsalPaths`, and the
project checks. The engine keeps an unchanged snapshot and migrates a
second copy. `Spec::rehearse_adoption` also reruns the legacy validator.
All successful artifacts remain available if a later step fails. Choose
fresh destination paths when retrying. The application owns retention.

Apply and rehearsal hold an exclusive advisory lock beside the canonical
database path. The lock file stays in place after release so concurrent
processes use the same lock. SQLite transactions exclude other writers
during migration. Participating migrators must use the same canonical
path. The engine checks foreign keys, database integrity, and the project
verifier before committing. A project that requires exact schema matching
supplies that check in its verifier.

The engine rejects `DROP TABLE` while foreign-key enforcement is on
because a table rebuild can cascade-delete dependent rows. Production
integration, table rebuild support, repeatable SQL, and down migrations
have their own follow-up work.
