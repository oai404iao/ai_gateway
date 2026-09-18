//! File-backed SQLite primitive contracts; these are not backend feature-parity tests.

#![cfg(feature = "sqlite-backend")]

use std::{path::Path, str::FromStr, sync::Arc, time::Duration};

use ai_gateway::{
    persistence::sqlite::{SqliteDatabase, SqliteDecimal},
    runtime_config::AppConfig,
};
use rust_decimal::Decimal;
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use tokio::time::timeout;

async fn database() -> (tempfile::TempDir, SqliteDatabase) {
    let directory = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    (directory, db)
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
async fn foundation_does_not_enable_server_configuration_or_memory_databases() {
    for path in [":memory:", "", "gateway.sqlite", "/"] {
        assert!(SqliteDatabase::open(Path::new(path)).await.is_err());
    }
    let mut config: AppConfig = toml::from_str(include_str!("../config.example.toml")).unwrap();
    config.database.url = "sqlite:///gateway.sqlite".into();
    let error = config
        .validate()
        .err()
        .expect("SQLite must remain disabled");
    assert!(error.to_string().contains("database URL must use postgres"));
}
