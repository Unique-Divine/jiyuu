use evolve_sqlite::{
    CheckError, Error, Migration, RehearsalPaths, Spec, schema::Schema,
};
use rusqlite::Connection;
use tempfile::{NamedTempFile, tempdir};

type TestResult = Result<(), CheckError>;

const INITIAL: Migration<'_> = Migration {
    name: "01_initial.sql",
    sql: "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL DEFAULT 'Owner Text');",
};
const UPGRADE: Migration<'_> = Migration {
    name: "02_search.sql",
    sql: "CREATE INDEX notes_body ON notes(body); CREATE VIEW note_preview AS SELECT id,body FROM notes;",
};

/// A single committed schema reference checks fresh install and incremental
/// upgrade alike, including the rehearsal copy and final live transaction.
#[test]
fn one_reference_checks_fresh_upgrade_copy_and_live() -> TestResult {
    let expected =
        Schema::from_sql(&format!("{} {}", INITIAL.sql, UPGRADE.sql))?;
    let migrations = [INITIAL, UPGRADE];
    let target = Spec {
        app_id: 0x4556_4f4c,
        migrations: &migrations,
    };
    let first = Spec {
        app_id: target.app_id,
        migrations: &migrations[..1],
    };
    for upgrade in [false, true] {
        let db = NamedTempFile::new()?;
        if upgrade {
            first.apply(&first.plan(db.path())?, |_| Ok(()), |_, ()| Ok(()))?;
        }
        let dir = tempdir()?;
        let paths = RehearsalPaths {
            snapshot: dir.path().join("snapshot.db"),
            copy: dir.path().join("copy.db"),
        };
        target.apply_verified(
            &target.plan(db.path())?,
            &paths,
            |_| Ok(()),
            |conn, ()| {
                expected.verify(conn)?;
                Ok(())
            },
        )?;
        expected.verify(&Connection::open(db.path())?)?;
        expected.verify(&Connection::open(paths.copy)?)?;
    }
    Ok(())
}

/// Changing a default's case or whitespace is a real schema change. Identical
/// columns alone do not establish that a table has the intended constraints.
#[test]
fn detects_constraint_literal_and_object_inventory_drift() -> TestResult {
    let expected = Schema::from_sql(
        "CREATE TABLE config(value TEXT DEFAULT 'Owner Text' CHECK(length(value)>0)); CREATE VIEW config_view AS SELECT value FROM config;",
    )?;
    for sql in [
        "CREATE TABLE config(value TEXT DEFAULT 'owner text' CHECK(length(value)>0)); CREATE VIEW config_view AS SELECT value FROM config;",
        "CREATE TABLE config(value TEXT DEFAULT 'OwnerText' CHECK(length(value)>0)); CREATE VIEW config_view AS SELECT value FROM config;",
        "CREATE TABLE config(value TEXT DEFAULT 'Owner Text'); CREATE VIEW config_view AS SELECT value FROM config;",
        "CREATE TABLE config(value TEXT DEFAULT 'Owner Text' CHECK(length(value)>0));",
        "CREATE TABLE config(value TEXT DEFAULT 'Owner Text' CHECK(length(value)>0)); CREATE VIEW config_view AS SELECT value FROM config; CREATE TABLE unreviewed(value);",
    ] {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(sql)?;
        assert!(matches!(expected.verify(&conn), Err(Error::Drift(_))));
    }
    Ok(())
}

/// Adopted files retain a history table absent from fresh files. The caller
/// must supply its exact expected DDL rather than hiding arbitrary extra tables.
#[test]
fn legacy_history_requires_an_explicit_reference_definition() -> TestResult {
    let fresh = Schema::from_sql(INITIAL.sql)?;
    let legacy_ddl = "CREATE TABLE old_history(name TEXT PRIMARY KEY, checksum TEXT NOT NULL);";
    let adopted = Schema::from_sql(&format!("{} {legacy_ddl}", INITIAL.sql))?;
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(INITIAL.sql)?;
    fresh.verify(&conn)?;
    conn.execute_batch(legacy_ddl)?;
    assert!(matches!(fresh.verify(&conn), Err(Error::Drift(_))));
    adopted.verify(&conn)?;
    conn.execute_batch("ALTER TABLE old_history ADD COLUMN forged INTEGER;")?;
    assert!(matches!(adopted.verify(&conn), Err(Error::Drift(_))));
    Ok(())
}

/// SQLite creates FTS shadow tables from the virtual-table declaration. Both
/// references and application files must compare that complete generated schema.
#[test]
fn fts_shadow_objects_and_triggers_are_part_of_the_reference() -> TestResult {
    let ddl = "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT);
        CREATE VIRTUAL TABLE search USING fts5(body, content='notes', content_rowid='id');
        CREATE TRIGGER notes_insert AFTER INSERT ON notes BEGIN INSERT INTO search(rowid,body) VALUES(new.id,new.body); END;";
    let expected = Schema::from_sql(ddl)?;
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(ddl)?;
    expected.verify(&conn)?;
    conn.execute_batch("INSERT INTO notes VALUES(1,'customized text');")?;
    expected.verify(&conn)?;
    conn.execute_batch("DROP TRIGGER notes_insert;")?;
    assert!(matches!(expected.verify(&conn), Err(Error::Drift(_))));
    Ok(())
}

/// Schema comparison remains read-only inside the verifier. A mismatch rolls
/// back pending SQL and its history, leaving the reviewed plan usable for retry.
#[test]
fn schema_mismatch_rejects_the_whole_apply() -> TestResult {
    let expected = Schema::from_sql("CREATE TABLE wrong(id);")?;
    let db = NamedTempFile::new()?;
    let migrations = [INITIAL];
    let target = Spec {
        app_id: 0x4556_4f4c,
        migrations: &migrations,
    };
    let plan = target.plan(db.path())?;
    assert!(matches!(
        target.apply(
            &plan,
            |_| Ok(()),
            |conn, ()| {
                expected.verify(conn)?;
                Ok(())
            }
        ),
        Err(Error::Verification(_))
    ));
    assert_eq!(target.plan(db.path())?, plan);
    Ok(())
}

/// SQLite accepts an unresolved view expression X '4142' as an identifier and
/// alias. It must not compare equal to the single blob literal token X'4142'.
#[test]
fn blob_literal_is_distinct_from_identifier_and_string() -> TestResult {
    let expected = Schema::from_sql("CREATE VIEW sample AS SELECT X'4142';")?;
    for sql in [
        "CREATE VIEW sample AS SELECT X '4142';",
        "CREATE VIEW sample AS SELECT X/**/'4142';",
    ] {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(sql)?;
        assert!(matches!(expected.verify(&conn), Err(Error::Drift(_))));
    }
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("CREATE VIEW sample AS SELECT x'4142';")?;
    expected.verify(&conn)?;
    assert!(
        Schema::from_sql("CREATE VIEW invalid AS SELECT X'not hex';").is_err()
    );
    Ok(())
}

/// Whitespace or a comment can separate a numeric exponent into an alias.
/// SQLite accepts both views with the same column name but different values.
#[test]
fn numeric_literal_boundaries_detect_changed_view_results() -> TestResult {
    for (literal, separated, expected_value, actual_value) in [
        ("1.e2", "1. e2", 100.0, 1.0),
        ("1.e2", "1./**/e2", 100.0, 1.0),
        (".5e2", ".5 e2", 50.0, 0.5),
        (".5e2", ".5/**/e2", 50.0, 0.5),
    ] {
        let expected_sql =
            format!("CREATE VIEW sample(value) AS SELECT {literal};");
        let expected = Schema::from_sql(&expected_sql)?;
        let reference = Connection::open_in_memory()?;
        reference.execute_batch(&expected_sql)?;
        assert_eq!(
            reference.query_row("SELECT value FROM sample", [], |r| r
                .get::<_, f64>(0))?,
            expected_value
        );
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(&format!(
            "CREATE VIEW sample(value) AS SELECT {separated};"
        ))?;
        assert_eq!(
            conn.query_row("SELECT value FROM sample", [], |r| r
                .get::<_, f64>(0))?,
            actual_value
        );
        assert!(matches!(expected.verify(&conn), Err(Error::Drift(_))));
    }
    // Validate supported syntax using the same bundled SQLite as comparison.
    for literal in [
        ".5",
        "1.",
        "1e+2",
        "1.e-2",
        "0xAb",
        "1_000",
        "0xDE_AD",
        "1_2.3_4e+5_6",
    ] {
        let ddl = format!("CREATE VIEW sample(value) AS SELECT {literal};");
        let expected = Schema::from_sql(&ddl)?;
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(&ddl)?;
        expected.verify(&conn)?;
    }
    Ok(())
}
