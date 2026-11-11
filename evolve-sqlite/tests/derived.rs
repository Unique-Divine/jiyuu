use evolve_sqlite::{
    CheckError, DerivedSql, Error, Migration, RebuildPolicy, RehearsalPaths,
    Spec, schema::Schema,
};
use rusqlite::Connection;
use tempfile::{NamedTempFile, tempdir};

type TestResult = Result<(), CheckError>;

const INITIAL: Migration<'_> = Migration {
    name: "01_notes.sql",
    sql: "CREATE TABLE note(id INTEGER PRIMARY KEY, body TEXT NOT NULL);
          CREATE VIRTUAL TABLE note_fts USING fts5(body, content='note', content_rowid='id');
          INSERT INTO note VALUES(1, 'custom owner description');",
};
const VIEWS: DerivedSql<'_> = DerivedSql {
    name: "note_views",
    sql: "DROP VIEW IF EXISTS note_preview; CREATE VIEW note_preview AS SELECT id, body FROM note;",
    fts_rebuilds: &[],
};
const SEARCH: DerivedSql<'_> = DerivedSql {
    name: "note_search",
    sql: "DROP TRIGGER IF EXISTS note_ai;
          CREATE TRIGGER note_ai AFTER INSERT ON note BEGIN INSERT INTO note_fts(rowid,body) VALUES(new.id,new.body); END;",
    fts_rebuilds: &["note_fts"],
};

fn spec() -> Spec<'static> {
    Spec {
        application_id: 0x4556_4f4c,
        migrations: &[INITIAL],
    }
}

/// Replacing FTS maintenance triggers must rebuild the existing search index
/// in the same transaction, preserving customized source rows on copy and live.
#[test]
fn derived_definitions_rehearse_and_rebuild_search_atomically() -> TestResult {
    let db = NamedTempFile::new()?;
    let definitions = [VIEWS, SEARCH];
    let target = spec();
    let policy = RebuildPolicy::default();
    let plan = target.plan_with_derived(db.path(), &policy, &definitions)?;
    assert_eq!(plan.pending_derived().count(), 2);
    let expected = Schema::from_sql(&format!(
        "{} {} {}",
        INITIAL.sql, VIEWS.sql, SEARCH.sql
    ))?;
    let dir = tempdir()?;
    let paths = RehearsalPaths {
        snapshot: dir.path().join("snapshot.db"),
        copy: dir.path().join("copy.db"),
    };
    target.apply_verified(
        &plan,
        &paths,
        |_| Ok(()),
        |conn, ()| {
            expected.verify(conn)?;
            let found: i64 = conn.query_row(
                "SELECT rowid FROM note_fts WHERE note_fts MATCH 'custom'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(found, 1);
            Ok(())
        },
    )?;
    for path in [db.path(), paths.copy.as_path()] {
        let conn = Connection::open(path)?;
        assert_eq!(
            conn.query_row("SELECT body FROM note", [], |r| r
                .get::<_, String>(0))?,
            "custom owner description"
        );
        conn.execute("INSERT INTO note VALUES(2,'future note')", [])?;
        assert_eq!(
            conn.query_row(
                "SELECT rowid FROM note_fts WHERE note_fts MATCH 'future'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            2
        );
    }
    Ok(())
}

/// Reapplying an unchanged declaration must neither rerun its SQL nor restamp
/// history. Changed FTS target lists must produce pending work independently.
#[test]
fn no_op_preserves_timestamps_and_fts_targets_affect_checksum() -> TestResult {
    let db = NamedTempFile::new()?;
    let target = spec();
    let policy = RebuildPolicy::default();
    let declarations = [VIEWS];
    target.apply(
        &target.plan_with_derived(db.path(), &policy, &declarations)?,
        |_| Ok(()),
        |_, ()| Ok(()),
    )?;
    let conn = Connection::open(db.path())?;
    conn.execute("UPDATE evolve_sqlite_derived SET applied_at=123", [])?;
    let current = target.plan_with_derived(db.path(), &policy, &declarations)?;
    assert_eq!(current.pending_derived().count(), 0);
    target.apply(&current, |_| Ok(()), |_, ()| Ok(()))?;
    assert_eq!(
        conn.query_row(
            "SELECT applied_at FROM evolve_sqlite_derived",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        123
    );
    let changed = [DerivedSql {
        fts_rebuilds: &["note_fts"],
        ..VIEWS
    }];
    assert_eq!(
        target
            .plan_with_derived(db.path(), &policy, &changed)?
            .pending_derived()
            .count(),
        1
    );
    assert!(matches!(target.plan(db.path()), Err(Error::Ahead(_))));
    Ok(())
}

/// A changed view and its checksum are rolled back together if final project
/// verification rejects the result, retaining the old definition for retry.
#[test]
fn project_failure_rolls_back_derived_ddl_and_history() -> TestResult {
    let db = NamedTempFile::new()?;
    let target = spec();
    let policy = RebuildPolicy::default();
    target.apply(
        &target.plan_with_derived(db.path(), &policy, &[VIEWS])?,
        |_| Ok(()),
        |_, ()| Ok(()),
    )?;
    let changed = [DerivedSql {
        sql: "DROP VIEW note_preview; CREATE VIEW note_preview AS SELECT id FROM note;",
        ..VIEWS
    }];
    let plan = target.plan_with_derived(db.path(), &policy, &changed)?;
    assert!(matches!(
        target.apply(
            &plan,
            |_| Ok(()),
            |_, ()| Err("reject changed view".into())
        ),
        Err(Error::Verification(_))
    ));
    assert_eq!(
        target.plan_with_derived(db.path(), &policy, &changed)?,
        plan
    );
    let conn = Connection::open(db.path())?;
    assert_eq!(
        conn.query_row("SELECT body FROM note_preview", [], |r| r
            .get::<_, String>(0))?,
        "custom owner description"
    );
    Ok(())
}

/// Derived DDL must not rewrite business seeds, tables, engine history, or
/// transaction boundaries, even though it executes inside the engine batch.
#[test]
fn derived_sql_cannot_mutate_source_rows_or_engine_metadata() -> TestResult {
    for sql in [
        "UPDATE note SET body='erased';",
        "DROP TABLE note;",
        "CREATE TABLE seed(value);",
        "DELETE FROM evolve_sqlite_migration;",
        "DELETE FROM evolve_sqlite_derived;",
        "COMMIT;",
        "PRAGMA user_version=9;",
        "CREATE TRIGGER forged AFTER INSERT ON evolve_sqlite_derived BEGIN SELECT 1; END;",
    ] {
        let db = NamedTempFile::new()?;
        let target = spec();
        let declaration = [DerivedSql {
            name: "forbidden",
            sql,
            fts_rebuilds: &[],
        }];
        let plan = target.plan_with_derived(
            db.path(),
            &RebuildPolicy::default(),
            &declaration,
        )?;
        assert!(
            matches!(
                target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
                Err(Error::Derived { .. })
            ),
            "{sql}"
        );
        assert_eq!(
            target.plan_with_derived(
                db.path(),
                &RebuildPolicy::default(),
                &declaration
            )?,
            plan
        );
    }
    Ok(())
}

/// Rebuild commands may target only real FTS5 virtual tables. An ordinary table
/// with a similarly named column must never receive engine-generated inserts.
#[test]
fn invalid_fts_target_rejects_the_batch() -> TestResult {
    let db = NamedTempFile::new()?;
    let target = spec();
    let definition = [DerivedSql {
        fts_rebuilds: &["note"],
        ..VIEWS
    }];
    let plan = target.plan_with_derived(
        db.path(),
        &RebuildPolicy::default(),
        &definition,
    )?;
    assert!(matches!(
        target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::InvalidSpec(_))
    ));
    assert_eq!(
        target.plan_with_derived(
            db.path(),
            &RebuildPolicy::default(),
            &definition
        )?,
        plan
    );
    Ok(())
}

/// Declaration names and FTS targets are reviewable identifiers, not SQL text.
/// Duplicate names or targets must fail during the read-only planning step.
#[test]
fn validates_names_and_duplicates_before_apply() -> TestResult {
    let db = NamedTempFile::new()?;
    for definitions in [
        vec![VIEWS, VIEWS],
        vec![DerivedSql {
            name: "evolve_sqlite_derived",
            ..VIEWS
        }],
        vec![DerivedSql {
            fts_rebuilds: &["note_fts", "NOTE_FTS"],
            ..VIEWS
        }],
        vec![DerivedSql {
            fts_rebuilds: &["main.note_fts"],
            ..VIEWS
        }],
    ] {
        assert!(matches!(
            spec().plan_with_derived(
                db.path(),
                &RebuildPolicy::default(),
                &definitions
            ),
            Err(Error::InvalidSpec(_))
        ));
    }
    Ok(())
}

/// A writer changing tracked timestamps after planning invalidates the plan;
/// forged triggers on the derived history table are rejected before SQL runs.
#[test]
fn revalidates_derived_history_and_rejects_forged_triggers() -> TestResult {
    let db = NamedTempFile::new()?;
    let target = spec();
    let policy = RebuildPolicy::default();
    target.apply(
        &target.plan_with_derived(db.path(), &policy, &[VIEWS])?,
        |_| Ok(()),
        |_, ()| Ok(()),
    )?;
    let plan = target.plan_with_derived(db.path(), &policy, &[VIEWS])?;
    let conn = Connection::open(db.path())?;
    conn.execute("UPDATE evolve_sqlite_derived SET applied_at=123", [])?;
    assert!(matches!(
        target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::StalePlan)
    ));
    conn.execute_batch("CREATE TRIGGER forged AFTER UPDATE ON evolve_sqlite_derived BEGIN SELECT 1; END;")?;
    assert!(matches!(
        target.plan_with_derived(db.path(), &policy, &[VIEWS]),
        Err(Error::Drift(_))
    ));
    Ok(())
}

/// A failed FTS rebuild must roll back earlier view changes, versioned SQL,
/// and both history tables, even after the derived DDL itself succeeded.
#[test]
fn failed_fts_rebuild_rolls_back_all_pending_work() -> TestResult {
    let db = NamedTempFile::new()?;
    let migrations = [
        INITIAL,
        Migration {
            name: "02_broken_search.sql",
            sql: "CREATE VIRTUAL TABLE broken USING fts5(missing,content='note',content_rowid='id');",
        },
    ];
    let target = Spec {
        migrations: &migrations,
        ..spec()
    };
    let definitions = [DerivedSql {
        fts_rebuilds: &["broken"],
        ..VIEWS
    }];
    let plan = target.plan_with_derived(
        db.path(),
        &RebuildPolicy::default(),
        &definitions,
    )?;
    assert!(matches!(
        target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::Derived { .. })
    ));
    assert_eq!(
        target.plan_with_derived(
            db.path(),
            &RebuildPolicy::default(),
            &definitions
        )?,
        plan
    );
    Ok(())
}

/// Migration permissions must protect the additive derived-history table even
/// before the engine creates it, without changing frozen versioned history.
#[test]
fn versioned_sql_cannot_forge_derived_history() -> TestResult {
    let db = NamedTempFile::new()?;
    let migrations = [
        INITIAL,
        Migration {
            name: "02_forged_history.sql",
            sql: "CREATE TABLE evolve_sqlite_derived(name TEXT);",
        },
    ];
    let target = Spec {
        migrations: &migrations,
        ..spec()
    };
    let plan = target.plan(db.path())?;
    assert!(matches!(
        target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::Migration { .. })
    ));
    assert_eq!(target.plan(db.path())?, plan);
    Ok(())
}

/// The view/trigger definitions themselves are safe to replay. Repeating them
/// preserves source customizations, and rebuilding FTS retains search results.
#[test]
fn guarded_derived_sql_can_be_replayed_without_source_changes() -> TestResult {
    let db = NamedTempFile::new()?;
    let target = spec();
    let definitions = [VIEWS, SEARCH];
    target.apply(
        &target.plan_with_derived(
            db.path(),
            &RebuildPolicy::default(),
            &definitions,
        )?,
        |_| Ok(()),
        |_, ()| Ok(()),
    )?;
    let conn = Connection::open(db.path())?;
    let expected = Schema::from_sql(&format!(
        "{} {} {}",
        INITIAL.sql, VIEWS.sql, SEARCH.sql
    ))?;
    for _ in 0..3 {
        conn.execute_batch(VIEWS.sql)?;
        conn.execute_batch(SEARCH.sql)?;
        conn.execute("INSERT INTO note_fts(note_fts) VALUES('rebuild')", [])?;
        expected.verify(&conn)?;
        assert_eq!(
            conn.query_row("SELECT body FROM note WHERE id=1", [], |r| r
                .get::<_, String>(0))?,
            "custom owner description"
        );
        assert_eq!(
            conn.query_row(
                "SELECT rowid FROM note_fts WHERE note_fts MATCH 'custom'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
    }
    Ok(())
}

/// A changed content view may follow its maintenance trigger declaration.
/// Rebuilds must index the final view after every pending definition is applied.
#[test]
fn rebuilds_use_final_content_view_after_all_pending_ddl() -> TestResult {
    let db = NamedTempFile::new()?;
    let migrations = [Migration {
        name: "01_view_search.sql",
        sql: "CREATE TABLE note(id INTEGER PRIMARY KEY,title TEXT,body TEXT);
            INSERT INTO note VALUES(1,'oldtitle','currentbody');
            CREATE VIEW search_source AS SELECT id,title AS body FROM note;
            CREATE VIRTUAL TABLE search USING fts5(body,content='search_source',content_rowid='id');",
    }];
    let target = Spec {
        migrations: &migrations,
        ..spec()
    };
    let old = [
        DerivedSql {
            name: "maintenance",
            sql: "DROP TRIGGER IF EXISTS note_ai; CREATE TRIGGER note_ai AFTER INSERT ON note BEGIN INSERT INTO search(rowid,body) VALUES(new.id,new.title); END;",
            fts_rebuilds: &["search"],
        },
        DerivedSql {
            name: "content_view",
            sql: "DROP VIEW IF EXISTS search_source; CREATE VIEW search_source AS SELECT id,title AS body FROM note;",
            fts_rebuilds: &[],
        },
    ];
    let policy = RebuildPolicy::default();
    target.apply(
        &target.plan_with_derived(db.path(), &policy, &old)?,
        |_| Ok(()),
        |_, ()| Ok(()),
    )?;
    let conn = Connection::open(db.path())?;
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM search WHERE search MATCH 'oldtitle'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        1
    );
    let changed = [
        DerivedSql {
            sql: "DROP TRIGGER IF EXISTS note_ai; CREATE TRIGGER note_ai AFTER INSERT ON note BEGIN INSERT INTO search(rowid,body) VALUES(new.id,new.body); END;",
            ..old[0]
        },
        DerivedSql {
            sql: "DROP VIEW IF EXISTS search_source; CREATE VIEW search_source AS SELECT id,body FROM note;",
            fts_rebuilds: &["SEARCH"],
            ..old[1]
        },
    ];
    let plan = target.plan_with_derived(db.path(), &policy, &changed)?;
    assert_eq!(plan.pending_derived().count(), 2);
    target.apply(&plan, |_| Ok(()), |_, ()| Ok(()))?;
    assert_eq!(
        conn.query_row("SELECT body FROM search_source WHERE id=1", [], |r| {
            r.get::<_, String>(0)
        })?,
        "currentbody"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM search WHERE search MATCH 'currentbody'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM search WHERE search MATCH 'oldtitle'",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        0
    );
    Ok(())
}

/// Maintenance triggers belong to ordinary source tables. A benign trigger
/// declaration on shadow metadata must fail before any trigger can execute.
#[test]
fn rejects_benign_shadow_trigger_declarations_before_rebuild() -> TestResult {
    let db = NamedTempFile::new()?;
    let declaration = [DerivedSql {
        name: "unsupported_trigger",
        sql: "CREATE TRIGGER helper AFTER INSERT ON note_fts_data BEGIN SELECT 1; END;",
        fts_rebuilds: &["note_fts"],
    }];
    let target = spec();
    let policy = RebuildPolicy::default();
    let plan = target.plan_with_derived(db.path(), &policy, &declaration)?;
    assert!(matches!(
        target.apply(&plan, |_| Ok(()), |_, ()| Ok(())),
        Err(Error::Derived { .. })
    ));
    assert_eq!(
        target.plan_with_derived(db.path(), &policy, &declaration)?,
        plan
    );
    Ok(())
}
