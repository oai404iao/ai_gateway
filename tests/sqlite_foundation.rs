//! File-backed SQLite primitive contracts; these are not backend feature-parity tests.

#![cfg(all(feature = "sqlite-backend", target_os = "linux"))]

use std::{os::unix::fs::PermissionsExt, path::Path, str::FromStr, sync::Arc, time::Duration};

use ai_gateway::persistence::{DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID};
use ai_gateway::{
    persistence::sqlite::{
        SqliteDatabase, SqliteDecimal, SqliteMigration, SqliteMigrationError, SqliteOpenError,
    },
    runtime_config::AppConfig,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use tokio::time::timeout;

#[path = "contracts/sqlite_auth.rs"]
mod sqlite_auth;
#[path = "contracts/sqlite_control_plane.rs"]
mod sqlite_control_plane;
#[path = "contracts/sqlite_pipeline.rs"]
mod sqlite_pipeline;
#[path = "contracts/sqlite_queries.rs"]
mod sqlite_queries;
#[path = "contracts/sqlite_schema.rs"]
mod sqlite_schema;
#[path = "contracts/sqlite_upstream_identity_migration.rs"]
mod sqlite_upstream_identity_migration;

async fn database() -> (tempfile::TempDir, SqliteDatabase) {
    let directory = private_directory();
    let db = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    (directory, db)
}

fn private_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

async fn create_table(db: &SqliteDatabase) {
    let mut tx = db.begin_write().await.unwrap();
    sqlx::query("CREATE TABLE amounts (id INTEGER PRIMARY KEY, amount TEXT NOT NULL) STRICT")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn connection_policy_applies_to_all_readers_and_replacements() {
    let (_directory, db) = database().await;
    create_table(&db).await;
    let mut readers = Vec::new();
    for _ in 0..4 {
        let mut connection = db.acquire_read().await.unwrap();
        for (pragma, expected) in [
            ("PRAGMA synchronous", 2_i64),
            ("PRAGMA foreign_keys", 1),
            ("PRAGMA recursive_triggers", 1),
            ("PRAGMA read_uncommitted", 0),
            ("PRAGMA query_only", 1),
            ("PRAGMA busy_timeout", 5000),
        ] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(pragma)
                    .fetch_one(&mut *connection)
                    .await
                    .unwrap(),
                expected
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
                .fetch_one(&mut *connection)
                .await
                .unwrap(),
            "wal"
        );
        assert!(
            sqlx::query("INSERT INTO amounts VALUES (1, '1')")
                .execute(&mut *connection)
                .await
                .is_err()
        );
        readers.push(connection);
    }
    for reader in readers {
        reader.close().await.unwrap();
    }
    let mut replacement = db.acquire_read().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA query_only")
            .fetch_one(&mut *replacement)
            .await
            .unwrap(),
        1
    );
    drop(replacement);
    db.close().await;
}

#[tokio::test]
async fn writer_serializes_and_drop_rolls_back_before_reuse() {
    let (_directory, db) = database().await;
    create_table(&db).await;
    let mut first = db.begin_write().await.unwrap();
    sqlx::query("INSERT INTO amounts VALUES (1, '10')")
        .execute(&mut *first)
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(50), db.begin_write())
            .await
            .is_err()
    );
    let mut reader = db.acquire_read().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM amounts")
            .fetch_one(&mut *reader)
            .await
            .unwrap(),
        0
    );
    drop(reader);
    drop(first);
    let mut next = timeout(Duration::from_secs(2), db.begin_write())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("INSERT INTO amounts VALUES (1, '20')")
        .execute(&mut *next)
        .await
        .unwrap();
    next.commit().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn cancelled_writer_rolls_back_and_releases_connection() {
    let (_directory, db) = database().await;
    create_table(&db).await;
    let db = Arc::new(db);
    let worker_db = Arc::clone(&db);
    let (ready, started) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        let mut tx = worker_db.begin_write().await.unwrap();
        sqlx::query("INSERT INTO amounts VALUES (1, '1')")
            .execute(&mut *tx)
            .await
            .unwrap();
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
        tx.commit().await.unwrap();
    });
    timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    let mut tx = timeout(Duration::from_secs(2), db.begin_write())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM amounts")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        0
    );
    tx.rollback().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn cancelled_begin_waiting_on_sqlite_lock_does_not_leak_a_transaction() {
    let (directory, db) = database().await;
    create_table(&db).await;
    let mut blocker = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(directory.path().join("gateway.sqlite")),
    )
    .await
    .unwrap();
    let lock = blocker.begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(
        timeout(Duration::from_millis(50), db.begin_write())
            .await
            .is_err()
    );
    lock.rollback().await.unwrap();
    let mut tx = timeout(Duration::from_secs(2), db.begin_write())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("INSERT INTO amounts VALUES (1, '1')")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let mut next = db.begin_write().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM amounts")
            .fetch_one(&mut *next)
            .await
            .unwrap(),
        0
    );
    next.rollback().await.unwrap();
    blocker.close().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn wal_reader_snapshot_survives_commit_and_file_reopens() {
    let (directory, db) = database().await;
    create_table(&db).await;
    let mut reader = db.acquire_read().await.unwrap();
    let mut snapshot = reader.begin().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM amounts")
            .fetch_one(&mut *snapshot)
            .await
            .unwrap(),
        0
    );
    let mut write = db.begin_write().await.unwrap();
    sqlx::query("INSERT INTO amounts VALUES (1, '1234567890123456.12345678')")
        .execute(&mut *write)
        .await
        .unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM amounts")
            .fetch_one(&mut *snapshot)
            .await
            .unwrap(),
        0
    );
    snapshot.commit().await.unwrap();
    drop(reader);
    db.close().await;
    let reopened = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    let mut reader = reopened.acquire_read().await.unwrap();
    let stored: SqliteDecimal = sqlx::query_scalar("SELECT amount FROM amounts")
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    assert_eq!(
        stored.0,
        Decimal::from_str("1234567890123456.12345678").unwrap()
    );
    drop(reader);
    reopened.close().await;
}

#[tokio::test]
async fn foreign_key_failure_rolls_back_schema_and_data_together() {
    let (_directory, db) = database().await;
    let mut tx = db.begin_write().await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE parents (id INTEGER PRIMARY KEY) STRICT;
         CREATE TABLE children (id INTEGER REFERENCES parents(id)) STRICT;
         INSERT INTO parents VALUES (1);",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(
        sqlx::query("INSERT INTO children VALUES (2)")
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut reader = db.acquire_read().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_schema WHERE name IN ('parents', 'children')"
        )
        .fetch_one(&mut *reader)
        .await
        .unwrap(),
        0
    );
    drop(reader);
    db.close().await;
}

#[tokio::test]
async fn decimals_round_trip_exactly_without_sqlite_numeric_affinity() {
    let (_directory, db) = database().await;
    create_table(&db).await;
    let mut tx = db.begin_write().await.unwrap();
    for (id, text) in [
        "0",
        "-0.00000000",
        "1.23000000",
        "0.00000001",
        "0.000000000001",
        "9999999999999999.99999999",
        "-9999999999999999.99999999",
        "999999999999.999999999999",
        "999999999999.99999999",
        "79228162514264337593543950335",
        "0.0000000000000000000000000001",
    ]
    .into_iter()
    .enumerate()
    {
        let expected = Decimal::from_str_exact(text).unwrap();
        sqlx::query("INSERT INTO amounts VALUES (?, ?)")
            .bind(id as i64)
            .bind(SqliteDecimal(expected))
            .execute(&mut *tx)
            .await
            .unwrap();
        let row = sqlx::query("SELECT amount, typeof(amount) AS storage FROM amounts WHERE id = ?")
            .bind(id as i64)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("storage"), "text");
        assert_eq!(row.get::<SqliteDecimal, _>("amount").0, expected);
        assert_eq!(
            row.get::<String, _>("amount"),
            expected.normalize().to_string()
        );
    }
    tx.commit().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn decimal_decoder_rejects_corruption_and_lossy_storage() {
    let (_directory, db) = database().await;
    let mut reader = db.acquire_read().await.unwrap();
    for invalid in [
        "",
        "NaN",
        "1e2",
        "1_000",
        " 1",
        "+1",
        "01",
        "-0",
        "1.0",
        "79228162514264337593543950336",
        "0.00000000000000000000000000001",
    ] {
        assert!(
            sqlx::query_scalar::<_, SqliteDecimal>("SELECT ?")
                .bind(invalid)
                .fetch_one(&mut *reader)
                .await
                .is_err()
        );
    }
    for query in ["SELECT 1", "SELECT 1.1", "SELECT x'31'", "SELECT NULL"] {
        assert!(
            sqlx::query_scalar::<_, SqliteDecimal>(query)
                .fetch_one(&mut *reader)
                .await
                .is_err()
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, Option<SqliteDecimal>>("SELECT NULL")
            .fetch_one(&mut *reader)
            .await
            .unwrap(),
        None
    );
    drop(reader);
    db.close().await;
}

#[tokio::test]
async fn server_configuration_accepts_files_but_never_memory_databases() {
    for path in [":memory:", "", "gateway.sqlite", "/"] {
        assert!(SqliteDatabase::open(Path::new(path)).await.is_err());
    }
    let mut config: AppConfig = toml::from_str(include_str!("../config.example.toml")).unwrap();
    config.database.url = "sqlite:///gateway.sqlite".into();
    config.database.password_file = None;
    assert!(config.validate().is_ok());
    assert!(unsafe { libsqlite3_sys::sqlite3_libversion_number() } >= 3_051_003);
}

const TEST_MIGRATION: SqliteMigration<'static> = SqliteMigration {
    version: 1,
    description: "test-only fixture, not the gateway business schema",
    sql: "CREATE TABLE migration_fixture (id INTEGER PRIMARY KEY) STRICT;
          INSERT INTO migration_fixture VALUES (1);",
};

async fn table_exists(db: &SqliteDatabase, name: &str) -> bool {
    let mut reader = db.acquire_read().await.unwrap();
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name=?")
        .bind(name)
        .fetch_one(&mut *reader)
        .await
        .unwrap()
        == 1
}

#[tokio::test]
async fn all_pending_migrations_and_history_rollback_together() {
    let (_directory, db) = database().await;
    let failed = [
        TEST_MIGRATION,
        SqliteMigration {
            version: 2,
            description: "deliberate constraint failure",
            sql: "INSERT INTO migration_fixture VALUES (1);",
        },
    ];
    assert!(db.migrate(&failed).await.is_err());
    assert!(!table_exists(&db, "migration_fixture").await);
    let mut reader = db.acquire_read().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _gateway_sqlite_migrations")
            .fetch_one(&mut *reader)
            .await
            .unwrap(),
        0
    );
    drop(reader);
    assert_eq!(db.migrate(&[TEST_MIGRATION]).await.unwrap(), 1);
    assert_eq!(db.migrate(&[TEST_MIGRATION]).await.unwrap(), 0);
    db.close().await;
}

#[tokio::test]
async fn migration_history_requires_an_exact_known_prefix() {
    let (_directory, db) = database().await;
    db.migrate(&[TEST_MIGRATION]).await.unwrap();
    for manifest in [
        vec![],
        vec![SqliteMigration {
            sql: "SELECT 1;",
            ..TEST_MIGRATION
        }],
        vec![SqliteMigration {
            description: "different description",
            ..TEST_MIGRATION
        }],
    ] {
        assert!(matches!(
            db.migrate(&manifest).await,
            Err(SqliteMigrationError::HistoryMismatch)
        ));
    }
    let mut tx = db.begin_write().await.unwrap();
    sqlx::query("UPDATE _gateway_sqlite_migrations SET version=2")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        db.migrate(&[TEST_MIGRATION]).await,
        Err(SqliteMigrationError::HistoryMismatch)
    ));
    db.close().await;
}

#[tokio::test]
async fn invalid_and_nontransactional_manifests_are_rejected() {
    let (_directory, db) = database().await;
    for migration in [
        SqliteMigration {
            version: 2,
            ..TEST_MIGRATION
        },
        SqliteMigration {
            sql: "-- no-transaction\nSELECT 1;",
            ..TEST_MIGRATION
        },
        SqliteMigration {
            description: "",
            ..TEST_MIGRATION
        },
        SqliteMigration {
            sql: "",
            ..TEST_MIGRATION
        },
    ] {
        assert!(matches!(
            db.migrate(&[migration]).await,
            Err(SqliteMigrationError::InvalidManifest)
        ));
    }
    db.close().await;
}

#[tokio::test]
async fn migration_sql_cannot_commit_a_partial_batch() {
    let (_directory, db) = database().await;
    for sql in [
        "COMMIT; CREATE TABLE escaped (id INTEGER) STRICT;",
        "ROLLBACK; BEGIN; CREATE TABLE escaped (id INTEGER) STRICT;",
    ] {
        assert!(
            db.migrate(&[
                TEST_MIGRATION,
                SqliteMigration {
                    version: 2,
                    description: "unexpected transaction control in migration SQL",
                    sql,
                },
            ])
            .await
            .is_err()
        );
        assert!(!table_exists(&db, "migration_fixture").await);
        assert!(!table_exists(&db, "escaped").await);
    }
    assert_eq!(db.migrate(&[TEST_MIGRATION]).await.unwrap(), 1);
    db.close().await;
}

#[tokio::test]
async fn deferred_constraint_commit_failure_rolls_back_migration_history() {
    let (_directory, db) = database().await;
    let migration = SqliteMigration {
        version: 1,
        description: "deferred constraint failure at commit",
        sql: "CREATE TABLE parents (id INTEGER PRIMARY KEY) STRICT;
              CREATE TABLE children (
                id INTEGER REFERENCES parents(id) DEFERRABLE INITIALLY DEFERRED
              ) STRICT;
              INSERT INTO children VALUES (1);",
    };
    assert!(db.migrate(&[migration]).await.is_err());
    assert!(!table_exists(&db, "parents").await);
    assert!(!table_exists(&db, "children").await);
    assert_eq!(db.migrate(&[TEST_MIGRATION]).await.unwrap(), 1);
    db.close().await;
}

#[tokio::test]
async fn concurrent_migration_calls_apply_once() {
    let (_directory, db) = database().await;
    let manifest = [TEST_MIGRATION];
    let (left, right) = tokio::join!(db.migrate(&manifest), db.migrate(&manifest));
    assert_eq!(left.unwrap() + right.unwrap(), 1);
    db.close().await;
}

#[tokio::test]
async fn cancelled_migration_rolls_back_and_discards_its_commit_hook() {
    let (_directory, db) = database().await;
    let db = Arc::new(db);
    let mut tx = db.begin_write().await.unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (resume, paused) = std::sync::mpsc::sync_channel(1);
    let mut started = Some(started);
    tx.lock_handle()
        .await
        .unwrap()
        .set_update_hook(move |event| {
            if event.table == "migration_fixture"
                && let Some(started) = started.take()
            {
                started.send(()).unwrap();
                paused.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
    tx.rollback().await.unwrap();
    let task_db = Arc::clone(&db);
    let task = tokio::spawn(async move { task_db.migrate(&[TEST_MIGRATION]).await });
    timeout(Duration::from_secs(3), ready)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    resume.send(()).unwrap();
    let mut tx = timeout(Duration::from_secs(3), db.begin_write())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _gateway_sqlite_migrations")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    assert!(!table_exists(&db, "migration_fixture").await);
    assert_eq!(db.migrate(&[TEST_MIGRATION]).await.unwrap(), 1);
    db.close().await;
}

#[tokio::test]
async fn managed_identity_survives_reopen_and_cannot_be_updated() {
    let (directory, db) = database().await;
    let identity = db.database_id();
    let mut tx = db.begin_write().await.unwrap();
    for sql in [
        "UPDATE _gateway_sqlite_identity SET database_id='different'",
        "DELETE FROM _gateway_sqlite_identity",
    ] {
        assert!(sqlx::query(sql).execute(&mut *tx).await.is_err());
    }
    tx.rollback().await.unwrap();
    db.close().await;
    let reopened = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    assert_eq!(reopened.database_id(), identity);
    reopened.close().await;
}

#[tokio::test]
async fn concurrent_close_waits_for_logical_owner_before_immediate_reopen() {
    let (directory, db) = database().await;
    let mut db = Arc::new(db);
    for _ in 0..16 {
        let reader = db.acquire_read().await.unwrap();
        let closing_db = Arc::clone(&db);
        let first_close = tokio::spawn(async move { closing_db.close().await });
        tokio::task::yield_now().await;
        let closing_db = Arc::clone(&db);
        let second_close = tokio::spawn(async move { closing_db.close().await });
        tokio::task::yield_now().await;
        assert!(!first_close.is_finished());
        assert!(!second_close.is_finished());
        reader.close().await.unwrap();
        timeout(Duration::from_secs(3), first_close)
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(3), second_close)
            .await
            .unwrap()
            .unwrap();
        db = Arc::new(
            SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
                .await
                .unwrap(),
        );
    }
    db.close().await;
}

#[tokio::test]
async fn complete_bootstrap_marker_is_resumed_but_partial_markers_are_not_repaired() {
    for contents in ["d3363a55-15bf-4d7d-b5a0-5e2e9a4c0001", "", "d3363a55"] {
        let directory = private_directory();
        let path = directory.path().join("gateway.sqlite");
        let marker = directory.path().join("gateway.sqlite.identity");
        std::fs::write(&path, []).unwrap();
        std::fs::write(&marker, contents).unwrap();
        for file in [&path, &marker] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        if contents.len() == 36 {
            let db = SqliteDatabase::open(&path).await.unwrap();
            assert_eq!(db.database_id().to_string(), contents);
            db.close().await;
        } else {
            assert!(matches!(
                SqliteDatabase::open(&path).await,
                Err(SqliteOpenError::ForeignDatabase)
            ));
            assert!(std::fs::read(&path).unwrap().is_empty());
        }
        assert_eq!(std::fs::read_to_string(marker).unwrap(), contents);
    }
}

#[tokio::test]
async fn foreign_database_is_rejected_without_changing_journal_mode() {
    let directory = private_directory();
    let path = directory.path().join("foreign.sqlite");
    std::fs::File::create(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let options = SqliteConnectOptions::new().filename(&path);
    let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query("CREATE TABLE foreign_table (id INTEGER)")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(matches!(
        SqliteDatabase::open(&path).await,
        Err(SqliteOpenError::ForeignDatabase)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!path.with_file_name("foreign.sqlite-wal").exists());
}

#[tokio::test]
async fn newer_or_incomplete_managed_identity_is_rejected() {
    for corruption in [
        "PRAGMA user_version=2",
        "PRAGMA application_id=17",
        "DROP TABLE _gateway_sqlite_migrations",
        "DROP TRIGGER gateway_identity_no_update",
        "DROP TRIGGER gateway_identity_no_delete",
    ] {
        let (directory, db) = database().await;
        let mut tx = db.begin_write().await.unwrap();
        sqlx::query(corruption).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        db.close().await;
        assert!(matches!(
            SqliteDatabase::open(&directory.path().join("gateway.sqlite")).await,
            Err(SqliteOpenError::ForeignDatabase)
        ));
    }
}

#[tokio::test]
async fn private_directory_lock_covers_all_database_names() {
    let (directory, db) = database().await;
    for name in ["gateway.sqlite", "another.sqlite"] {
        assert!(matches!(
            SqliteDatabase::open(&directory.path().join(name)).await,
            Err(SqliteOpenError::AlreadyOwned)
        ));
    }
    assert!(!directory.path().join("another.sqlite").exists());
    db.close().await;
    let reopened = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    reopened.close().await;
}

#[tokio::test]
async fn outstanding_connection_keeps_process_lease_after_database_drop() {
    let (directory, db) = database().await;
    let reader = db.acquire_read().await.unwrap();
    drop(db);
    assert!(matches!(
        SqliteDatabase::open(&directory.path().join("gateway.sqlite")).await,
        Err(SqliteOpenError::AlreadyOwned)
    ));
    reader.close().await.unwrap();
    let reopened = timeout(Duration::from_secs(3), async {
        loop {
            match SqliteDatabase::open(&directory.path().join("gateway.sqlite")).await {
                Ok(database) => break database,
                Err(SqliteOpenError::AlreadyOwned) => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => panic!("unexpected reopen failure: {error}"),
            }
        }
    })
    .await
    .unwrap();
    reopened.close().await;
}

#[tokio::test]
async fn unsafe_permissions_and_file_aliases_are_rejected_without_repair() {
    use std::os::unix::fs::symlink;

    let directory = private_directory();
    let path = directory.path().join("gateway.sqlite");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        SqliteDatabase::open(&path).await,
        Err(SqliteOpenError::UnsafePath)
    ));
    assert!(!path.exists());
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::File::create(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        SqliteDatabase::open(&path).await,
        Err(SqliteOpenError::UnsafePath)
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias_directory = private_directory();
    let alias = alias_directory.path().join("gateway.sqlite");
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(matches!(
        SqliteDatabase::open(&alias).await,
        Err(SqliteOpenError::UnsafePath)
    ));
    let symlink_directory = private_directory();
    symlink(&path, symlink_directory.path().join("gateway.sqlite")).unwrap();
    assert!(
        SqliteDatabase::open(&symlink_directory.path().join("gateway.sqlite"))
            .await
            .is_err()
    );
    symlink(directory.path(), symlink_directory.path().join("alias")).unwrap();
    assert!(matches!(
        SqliteDatabase::open(&symlink_directory.path().join("alias/gateway.sqlite")).await,
        Err(SqliteOpenError::UnsafePath)
    ));
}

#[tokio::test]
async fn unsafe_sidecars_are_rejected_before_sqlite_opens_them() {
    use std::os::unix::fs::symlink;

    for suffix in ["-wal", "-shm", "-journal"] {
        let directory = private_directory();
        let target_directory = private_directory();
        let target = target_directory.path().join("target");
        std::fs::write(&target, b"must not change").unwrap();
        symlink(
            &target,
            directory.path().join(format!("gateway.sqlite{suffix}")),
        )
        .unwrap();
        assert!(matches!(
            SqliteDatabase::open(&directory.path().join("gateway.sqlite")).await,
            Err(SqliteOpenError::UnsafePath)
        ));
        assert_eq!(std::fs::read(target).unwrap(), b"must not change");
    }
}

#[tokio::test]
async fn path_replacement_fences_the_live_database_even_if_restored() {
    let (directory, db) = database().await;
    let original = directory.path().join("gateway.sqlite");
    let moved = directory.path().join("original.sqlite");
    std::fs::rename(&original, &moved).unwrap();
    std::fs::File::create(&original).unwrap();
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        db.acquire_read().await,
        Err(SqliteOpenError::IdentityChanged)
    ));
    std::fs::rename(&moved, &original).unwrap();
    assert!(matches!(
        db.begin_write().await,
        Err(SqliteOpenError::IdentityChanged)
    ));
    db.close().await;
}

#[test]
fn sqlite_owner_child_process() {
    let Some(path) = std::env::var_os("AI_GATEWAY_SQLITE_OWNER_TEST") else {
        return;
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let path = std::path::PathBuf::from(path);
        if std::env::var_os("AI_GATEWAY_SQLITE_EXPECT_OWNED").is_some() {
            assert!(matches!(
                SqliteDatabase::open(&path).await,
                Err(SqliteOpenError::AlreadyOwned)
            ));
            return;
        }
        let db = SqliteDatabase::open(&path).await.unwrap();
        db.migrate(&[TEST_MIGRATION]).await.unwrap();
        let mut tx = db.begin_write().await.unwrap();
        sqlx::query("CREATE TABLE interrupted_schema (id INTEGER) STRICT")
            .execute(&mut *tx)
            .await
            .unwrap();
        std::fs::write(path.with_file_name("ready"), db.database_id().to_string()).unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
        tx.rollback().await.unwrap();
        db.close().await;
    });
}

#[tokio::test]
async fn process_lease_remains_after_pool_close_and_cancelled_replacement() {
    let (directory, db) = database().await;
    let reader = db.acquire_read().await.unwrap();
    reader.close().await.unwrap();
    let _ = timeout(Duration::ZERO, db.acquire_read()).await;
    db.close().await;
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sqlite_owner_child_process", "--nocapture"])
        .env(
            "AI_GATEWAY_SQLITE_OWNER_TEST",
            directory.path().join("gateway.sqlite"),
        )
        .env("AI_GATEWAY_SQLITE_EXPECT_OWNED", "1")
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn unmarked_wal_database_is_rejected_without_touching_any_sidecar() {
    let directory = private_directory();
    let path = directory.path().join("foreign.sqlite");
    std::fs::File::create(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
    let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query("CREATE TABLE foreign_table (id INTEGER)")
        .execute(&mut connection)
        .await
        .unwrap();
    let files = [
        path.clone(),
        path.with_file_name("foreign.sqlite-wal"),
        path.with_file_name("foreign.sqlite-shm"),
    ];
    let contents: Vec<_> = files
        .iter()
        .map(|file| std::fs::read(file).unwrap())
        .collect();
    assert!(matches!(
        SqliteDatabase::open(&path).await,
        Err(SqliteOpenError::ForeignDatabase)
    ));
    for (file, original) in files.iter().zip(contents) {
        assert_eq!(std::fs::read(file).unwrap(), original);
    }
    assert!(!path.with_file_name("foreign.sqlite.identity").exists());
    connection.close().await.unwrap();
}

#[tokio::test]
async fn process_death_releases_ownership_and_recovers_committed_history_only() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let directory = private_directory();
    let path = directory.path().join("gateway.sqlite");
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sqlite_owner_child_process", "--nocapture"])
            .env("AI_GATEWAY_SQLITE_OWNER_TEST", &path)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ready = path.with_file_name("ready");
    timeout(Duration::from_secs(5), async {
        while !ready.exists() {
            assert!(child.0.try_wait().unwrap().is_none(), "child exited early");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        SqliteDatabase::open(&path).await,
        Err(SqliteOpenError::AlreadyOwned)
    ));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let reopened = SqliteDatabase::open(&path).await.unwrap();
    assert_eq!(
        reopened.database_id().to_string(),
        std::fs::read_to_string(ready).unwrap()
    );
    assert_eq!(reopened.migrate(&[TEST_MIGRATION]).await.unwrap(), 0);
    assert!(table_exists(&reopened, "migration_fixture").await);
    assert!(!table_exists(&reopened, "interrupted_schema").await);
    reopened.close().await;
}

/// Narrow facade contracts: the shared constructors keep their signatures and
/// the SQLite development constructors dispatch the ordinary operations.
#[tokio::test]
async fn repository_facades_dispatch_ordinary_operations_to_sqlite() {
    use ai_gateway::persistence::{AuthRepository, ControlPlaneRepository};
    use uuid::Uuid;

    let (_directory, database) = database().await;
    assert_eq!(database.install_schema().await.unwrap(), 5);
    let database = Arc::new(database);

    let auth = AuthRepository::from_sqlite(Arc::clone(&database));
    assert!(
        auth.find_login_user("nobody@example.test")
            .await
            .unwrap()
            .is_none()
    );

    let control_plane = ControlPlaneRepository::from_sqlite(Arc::clone(&database));
    assert!(control_plane.load().await.unwrap().proxies.is_empty());
    control_plane
        .ensure_system_settings(system_settings())
        .await
        .unwrap();
    assert_eq!(
        control_plane
            .system_settings()
            .await
            .unwrap()
            .settings
            .api_hosts,
        vec!["https://gateway.example.test"]
    );

    assert!(control_plane.sharing_groups(None).await.unwrap().is_empty());
    assert!(
        control_plane
            .codex_credentials(Uuid::nil())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        control_plane
            .lock_codex_refresh(Uuid::nil())
            .await
            .unwrap()
            .is_none()
    );
}

/// The S4 facades keep the production `new(PgPool)` signatures, dispatch all
/// read/write operations to SQLite, and derive every narrow handle from the one
/// shared database instead of opening another pool.
#[tokio::test]
async fn s4_pipeline_and_query_facades_share_one_sqlite_database() {
    use ai_gateway::persistence::{
        ChannelGroupStatusWindow, DatabaseHealth, MeteringQueries, RequestLogFilter,
        RequestLogQueries, RequestLogRepository, SettlementRepository,
    };

    let (_directory, database) = database().await;
    assert_eq!(database.install_schema().await.unwrap(), 5);
    let database = Arc::new(database);

    let repository = RequestLogRepository::from_sqlite(Arc::clone(&database));
    let queries = repository.queries();
    let settlements = repository.settlements();
    let metering = repository.metering();

    // A second facade family derived from the same shared handle.
    let standalone_queries = RequestLogQueries::from_sqlite(Arc::clone(&database));
    let standalone_metering = MeteringQueries::from_sqlite(Arc::clone(&database));
    let standalone_settlements = SettlementRepository::from_sqlite(Arc::clone(&database));

    for filter in [RequestLogFilter {
        limit: 10,
        user_id: None,
        api_key_id: None,
        model: None,
        api_format: None,
        api_operation: None,
        outcome: None,
        started_after: None,
        started_before: None,
        billed: None,
    }] {
        assert!(queries.list_all(filter.clone()).await.unwrap().is_empty());
        assert!(
            standalone_queries
                .list_all(filter)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            queries
                .channel_group_status(ChannelGroupStatusWindow::Last24Hours)
                .await
                .unwrap()
                .groups
                .is_empty()
        );
    }

    let counts = metering.reconciliation_counts().await.unwrap();
    assert_eq!(
        (counts.unknown, counts.invalid, counts.account_mismatch),
        (0, 0, 0)
    );
    assert_eq!(
        standalone_metering
            .sharing_completed_costs(&[])
            .await
            .unwrap(),
        Vec::new()
    );
    assert!(settlements.settle_pending(8).await.unwrap().is_empty());
    assert!(
        standalone_settlements
            .settle_pending(8)
            .await
            .unwrap()
            .is_empty()
    );

    // Pool observation reads live counts without acquiring a connection.
    let health = DatabaseHealth::from_sqlite(Arc::clone(&database));
    assert!(health.size() >= 1);
    assert!(health.idle() <= health.size() as usize);
}

fn system_settings() -> ai_gateway::persistence::SystemSettingsInput {
    serde_json::from_value(serde_json::json!({
        "api_hosts": ["https://gateway.example.test"],
        "upstream": {
            "connect_timeout_seconds": 10,
            "response_header_timeout_seconds": 30,
            "stream_idle_timeout_seconds": 60
        },
        "passive_health": {"connection_failure_threshold": 3, "cooldown_seconds": 60},
        "session_affinity": {
            "enabled": false, "max_entries": 100000, "default_ttl_seconds": 3600, "rules": []
        },
        "codex": {
            "originator": "codex_cli_rs",
            "client_version": "0.1.0",
            "user_agent": "codex_cli_rs/0.1.0"
        }
    }))
    .unwrap()
}
