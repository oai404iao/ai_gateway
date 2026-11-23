use ai_gateway::persistence::{MIGRATOR, MigrationRunError, run_migrations};

use super::TestDatabase;

#[tokio::test]
async fn concurrent_initialization_and_reopen_leave_one_baseline_and_seed_set() {
    let database = TestDatabase::new_unmigrated().await;
    sqlx::raw_sql(include_str!(
        "../../deploy/postgres/init/001-observability.sql"
    ))
    .execute(&database.pool)
    .await
    .unwrap();
    let (first, second) = tokio::join!(
        run_migrations(&database.pool),
        run_migrations(&database.pool)
    );
    first.unwrap();
    second.unwrap();
    run_migrations(&database.pool).await.unwrap();
    assert_eq!(MIGRATOR.iter().count(), 1);
    let baseline = MIGRATOR.iter().next().unwrap();
    assert_eq!(baseline.version, 1);
    assert!(!baseline.no_tx);
    let history: Vec<(i64, bool, Vec<u8>)> =
        sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations")
            .fetch_all(&database.pool)
            .await
            .unwrap();
    assert_eq!(history, [(1, true, baseline.checksum.to_vec())]);
    let roles: Vec<String> =
        sqlx::query_scalar("SELECT system_role FROM user_groups ORDER BY system_role")
            .fetch_all(&database.pool)
            .await
            .unwrap();
    assert_eq!(roles, ["admin", "user"]);

    sqlx::query("UPDATE _sqlx_migrations SET checksum='other-release'")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(matches!(
        run_migrations(&database.pool).await,
        Err(MigrationRunError::IncompatibleDatabase)
    ));
    let checksum: Vec<u8> =
        sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version=1")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(checksum, b"other-release");
    database.cleanup().await;
}

#[tokio::test]
async fn unrecognized_database_is_not_stamped_or_modified() {
    let database = TestDatabase::new_unmigrated().await;
    sqlx::raw_sql("CREATE SCHEMA other_app; CREATE TABLE other_app.records AS SELECT 42 AS value")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(matches!(
        run_migrations(&database.pool).await,
        Err(MigrationRunError::IncompatibleDatabase)
    ));
    let state: (i32, bool) = sqlx::query_as(
        "SELECT value, to_regclass('public._sqlx_migrations') IS NULL FROM other_app.records",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(state, (42, true));
    database.cleanup().await;
}
