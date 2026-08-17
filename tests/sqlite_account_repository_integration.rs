#![cfg(feature = "sqlite-backend")]

use std::{str::FromStr, sync::Arc, time::Duration};

use ai_gateway::{
    domain::ConsoleSessionPurpose,
    persistence::{
        DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID, RepositoryError, SQLITE_MIGRATOR,
        SqliteAuthRepository, SqliteDecimal,
    },
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tempfile::TempDir;
use tokio::sync::Barrier;
use uuid::Uuid;

const ADMIN_ID: Uuid = Uuid::from_u128(0x601);
const USER_ID: Uuid = Uuid::from_u128(0x602);
const OTHER_USER_ID: Uuid = Uuid::from_u128(0x603);
const INACTIVE_USER_ID: Uuid = Uuid::from_u128(0x604);
const PASSWORDLESS_USER_ID: Uuid = Uuid::from_u128(0x605);
const SYSTEM_USER_ID: Uuid = Uuid::from_u128(0x606);
const DELETED_USER_ID: Uuid = Uuid::from_u128(0x607);
const SESSION_ID: Uuid = Uuid::from_u128(0x611);
const OTHER_SESSION_ID: Uuid = Uuid::from_u128(0x612);
const PASSWORD_SESSION_ID: Uuid = Uuid::from_u128(0x613);
const OTHER_PASSWORD_SESSION_ID: Uuid = Uuid::from_u128(0x614);
const NORMAL_SESSION_ID: Uuid = Uuid::from_u128(0x615);

const FAR_FUTURE: &str = "9999-01-02T03:04:05.678Z";
const TEMPORARY_EXPIRY: &str = "9998-02-03T04:05:06.789Z";
const OLD_HASH: &str = "$argon2id$old-password-hash";
const TEMPORARY_HASH: &str = "$argon2id$temporary-password-hash";
const PERMANENT_HASH: &str = "$argon2id$permanent-password-hash";
const PRE_REVOKED_AT: &str = "2099-01-01T00:00:00.000Z";
const EXACT_BALANCE: &str = "1234567890123456.12345678";

struct TestDatabase {
    _directory: TempDir,
    pool: SqlitePool,
}

async fn migrated_pool() -> TestDatabase {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("console-account.sqlite3");
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    SQLITE_MIGRATOR.run(&pool).await.unwrap();
    TestDatabase {
        _directory: directory,
        pool,
    }
}

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

#[allow(clippy::too_many_arguments)]
async fn insert_user(
    pool: &SqlitePool,
    id: Uuid,
    email: &str,
    display_name: &str,
    role: &str,
    status: &str,
    password_hash: Option<&str>,
    balance: &str,
) {
    let group_id = if role == "admin" {
        DEFAULT_ADMIN_GROUP_ID
    } else {
        DEFAULT_USER_GROUP_ID
    };
    sqlx::query(
        "INSERT INTO users \
         (id,email,display_name,role,status,password_hash,balance_amount,user_group_id) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
    )
    .bind(id)
    .bind(email)
    .bind(display_name)
    .bind(role)
    .bind(status)
    .bind(password_hash)
    .bind(SqliteDecimal::from(
        Decimal::from_str_exact(balance).unwrap(),
    ))
    .bind(group_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_session(
    pool: &SqlitePool,
    id: Uuid,
    user_id: Uuid,
    purpose: &str,
    revoked_at: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO user_sessions \
         (id,user_id,refresh_token_hash,created_at,last_seen_at,expires_at,revoked_at,purpose) \
         VALUES (?1,?2,?3,'2026-01-01T00:00:00.000Z', \
                 '2026-01-01T00:00:00.000Z',?4,?5,?6)",
    )
    .bind(id)
    .bind(user_id)
    .bind(format!("refresh-{id}").into_bytes())
    .bind(FAR_FUTURE)
    .bind(revoked_at)
    .bind(purpose)
    .execute(pool)
    .await
    .unwrap();
}

async fn set_temporary_password_state(pool: &SqlitePool, user_id: Uuid, auth_version: i64) {
    sqlx::query(
        "UPDATE users SET password_hash=?2,auth_version=?3,password_change_required=1, \
         temporary_password_issued_at='9998-01-01T00:00:00.000Z', \
         temporary_password_expires_at=?4 WHERE id=?1",
    )
    .bind(user_id)
    .bind(TEMPORARY_HASH)
    .bind(auth_version)
    .bind(TEMPORARY_EXPIRY)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn sqlite_account_repository_updates_profiles_and_permanent_passwords() {
    let database = migrated_pool().await;
    insert_user(
        &database.pool,
        USER_ID,
        "profile@example.test",
        "SQLite profile user",
        "user",
        "active",
        Some(OLD_HASH),
        EXACT_BALANCE,
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    let original = repository.profile(USER_ID).await.unwrap().unwrap();
    assert_eq!(
        original.balance_amount,
        Decimal::from_str(EXACT_BALANCE).unwrap()
    );
    assert_eq!(original.display_name, "SQLite profile user");
    assert!(repository.profile(OTHER_USER_ID).await.unwrap().is_none());

    let updated = repository
        .update_display_name(USER_ID, "Updated SQLite profile")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.display_name, "Updated SQLite profile");
    assert!(updated.updated_at > original.updated_at);
    assert_eq!(
        updated.updated_at,
        repository
            .profile(USER_ID)
            .await
            .unwrap()
            .unwrap()
            .updated_at
    );

    sqlx::query("UPDATE users SET status='suspended' WHERE id=?1")
        .bind(USER_ID)
        .execute(&database.pool)
        .await
        .unwrap();
    let suspended_updated_at = repository
        .profile(USER_ID)
        .await
        .unwrap()
        .unwrap()
        .updated_at;
    assert!(
        repository
            .update_display_name(USER_ID, "Must not change")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repository
            .profile(USER_ID)
            .await
            .unwrap()
            .unwrap()
            .updated_at,
        suspended_updated_at
    );
    sqlx::query("UPDATE users SET status='active' WHERE id=?1")
        .bind(USER_ID)
        .execute(&database.pool)
        .await
        .unwrap();
    insert_session(&database.pool, SESSION_ID, USER_ID, "normal", None).await;
    insert_session(
        &database.pool,
        OTHER_SESSION_ID,
        USER_ID,
        "normal",
        Some(PRE_REVOKED_AT),
    )
    .await;

    assert!(
        repository
            .change_password(USER_ID, PERMANENT_HASH)
            .await
            .unwrap()
    );
    let state = sqlx::query_as::<
        _,
        (
            String,
            i64,
            Option<DateTime<Utc>>,
            bool,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        ),
    >(
        "SELECT password_hash,auth_version,password_changed_at,password_change_required, \
                temporary_password_issued_at,temporary_password_expires_at \
         FROM users WHERE id=?1",
    )
    .bind(USER_ID)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(state.0, PERMANENT_HASH);
    assert_eq!(state.1, 2);
    assert!(state.2.is_some());
    assert!(!state.3);
    assert!(state.4.is_none());
    assert!(state.5.is_none());
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1",
        )
        .bind(SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_some()
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1",
        )
        .bind(OTHER_SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        Some(timestamp(PRE_REVOKED_AT))
    );

    set_temporary_password_state(&database.pool, USER_ID, 3).await;
    assert!(
        !repository
            .change_password(USER_ID, "must-not-replace")
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT password_hash FROM users WHERE id=?1")
            .bind(USER_ID)
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        TEMPORARY_HASH
    );
    assert!(
        !repository
            .change_password(OTHER_USER_ID, "missing")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn sqlite_account_repository_issues_redacted_temporary_passwords() {
    let database = migrated_pool().await;
    insert_user(
        &database.pool,
        ADMIN_ID,
        "admin@example.test",
        "SQLite administrator",
        "admin",
        "active",
        Some(OLD_HASH),
        "0",
    )
    .await;
    insert_user(
        &database.pool,
        USER_ID,
        "target@example.test",
        "SQLite temporary target",
        "user",
        "active",
        Some(OLD_HASH),
        EXACT_BALANCE,
    )
    .await;
    insert_user(
        &database.pool,
        INACTIVE_USER_ID,
        "inactive@example.test",
        "SQLite inactive target",
        "user",
        "suspended",
        Some(OLD_HASH),
        "0",
    )
    .await;
    insert_user(
        &database.pool,
        PASSWORDLESS_USER_ID,
        "passwordless@example.test",
        "SQLite passwordless target",
        "user",
        "active",
        None,
        "0",
    )
    .await;
    insert_user(
        &database.pool,
        SYSTEM_USER_ID,
        "system@example.test",
        "SQLite system target",
        "user",
        "active",
        Some(OLD_HASH),
        "0",
    )
    .await;
    sqlx::query("UPDATE users SET is_system=1 WHERE id=?1")
        .bind(SYSTEM_USER_ID)
        .execute(&database.pool)
        .await
        .unwrap();
    insert_user(
        &database.pool,
        DELETED_USER_ID,
        "deleted@example.test",
        "SQLite deleted target",
        "user",
        "active",
        Some(OLD_HASH),
        "0",
    )
    .await;
    sqlx::query("UPDATE users SET deleted_at='2026-01-01T00:00:00.000Z' WHERE id=?1")
        .bind(DELETED_USER_ID)
        .execute(&database.pool)
        .await
        .unwrap();
    insert_session(&database.pool, SESSION_ID, USER_ID, "normal", None).await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    assert!(matches!(
        repository
            .issue_temporary_password(
                ADMIN_ID,
                1,
                ADMIN_ID,
                TEMPORARY_HASH,
                Duration::from_secs(60)
            )
            .await,
        Err(RepositoryError::CannotResetSelf)
    ));
    assert!(matches!(
        repository
            .issue_temporary_password(
                ADMIN_ID,
                2,
                USER_ID,
                TEMPORARY_HASH,
                Duration::from_secs(60)
            )
            .await,
        Err(RepositoryError::NotFound)
    ));
    for target in [SYSTEM_USER_ID, DELETED_USER_ID] {
        assert!(matches!(
            repository
                .issue_temporary_password(
                    ADMIN_ID,
                    1,
                    target,
                    TEMPORARY_HASH,
                    Duration::from_secs(60),
                )
                .await,
            Err(RepositoryError::NotFound)
        ));
    }
    for target in [INACTIVE_USER_ID, PASSWORDLESS_USER_ID] {
        assert!(matches!(
            repository
                .issue_temporary_password(
                    ADMIN_ID,
                    1,
                    target,
                    TEMPORARY_HASH,
                    Duration::from_secs(60),
                )
                .await,
            Err(RepositoryError::TemporaryPasswordUnavailable)
        ));
    }

    let started_at = Utc::now();
    let issued = repository
        .issue_temporary_password(
            ADMIN_ID,
            1,
            USER_ID,
            TEMPORARY_HASH,
            Duration::from_secs(24 * 60 * 60),
        )
        .await
        .unwrap();
    let completed_at = Utc::now();
    assert_eq!(issued.user_id, USER_ID);
    assert!(issued.expires_at >= started_at + chrono::Duration::hours(24));
    assert!(issued.expires_at <= completed_at + chrono::Duration::hours(24));
    let state = sqlx::query_as::<
        _,
        (
            String,
            i64,
            bool,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        ),
    >(
        "SELECT password_hash,auth_version,password_change_required, \
                temporary_password_issued_at,temporary_password_expires_at \
         FROM users WHERE id=?1",
    )
    .bind(USER_ID)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(state.0, TEMPORARY_HASH);
    assert_eq!(state.1, 2);
    assert!(state.2);
    assert!(state.3.is_some());
    assert_eq!(
        state.4.unwrap().timestamp_micros(),
        issued.expires_at.timestamp_micros()
    );
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1",
        )
        .bind(SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_some()
    );

    let audit = sqlx::query_as::<
        _,
        (
            Option<Uuid>,
            String,
            Option<String>,
            String,
            Uuid,
            String,
            String,
            Option<String>,
        ),
    >(
        "SELECT actor_user_id,actor_type,actor_role,action,object_id, \
                before_redacted,after_redacted,correlation_id \
         FROM audit_logs WHERE action='issue_temporary_password'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit.0, Some(ADMIN_ID));
    assert_eq!(audit.1, "user");
    assert_eq!(audit.2.as_deref(), Some("admin"));
    assert_eq!(audit.3, "issue_temporary_password");
    assert_eq!(audit.4, USER_ID);
    let correlation_id = issued.correlation_id.to_string();
    assert_eq!(audit.7.as_deref(), Some(correlation_id.as_str()));
    let before: Value = serde_json::from_str(&audit.5).unwrap();
    let after: Value = serde_json::from_str(&audit.6).unwrap();
    let expected_keys = [
        "balance_amount",
        "can_reissue_invitation",
        "created_at",
        "default_api_key_policy_id",
        "deleted_at",
        "deleted_by",
        "display_name",
        "effective_api_key_policy_id",
        "email",
        "id",
        "password_change_required",
        "role",
        "status",
        "temporary_password_expires_at",
        "updated_at",
        "user_group_id",
        "user_group_system_role",
        "websocket_enabled",
    ];
    assert_eq!(
        before
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        expected_keys
    );
    assert!(before["balance_amount"].is_number());
    assert!(
        audit
            .5
            .contains(&format!("\"balance_amount\":{EXACT_BALANCE}"))
    );
    assert_eq!(before["password_change_required"], false);
    assert_eq!(after["password_change_required"], true);
    assert!(after["temporary_password_expires_at"].is_string());
    let combined = format!("{}{}", audit.5, audit.6);
    assert!(!combined.contains(OLD_HASH));
    assert!(!combined.contains(TEMPORARY_HASH));
    assert!(!combined.contains("password_hash"));
    assert!(!combined.contains("temporary_password_issued_at"));
}

#[tokio::test]
async fn sqlite_account_repository_serializes_temporary_password_completion() {
    let database = migrated_pool().await;
    insert_user(
        &database.pool,
        ADMIN_ID,
        "completion-admin@example.test",
        "SQLite completion admin",
        "admin",
        "active",
        Some(OLD_HASH),
        "0",
    )
    .await;
    insert_user(
        &database.pool,
        USER_ID,
        "completion@example.test",
        "SQLite completion user",
        "user",
        "active",
        Some(OLD_HASH),
        "1.25",
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());
    repository
        .issue_temporary_password(
            ADMIN_ID,
            1,
            USER_ID,
            TEMPORARY_HASH,
            Duration::from_secs(24 * 60 * 60),
        )
        .await
        .unwrap();
    insert_session(&database.pool, NORMAL_SESSION_ID, USER_ID, "normal", None).await;
    assert!(
        repository
            .complete_temporary_password(USER_ID, NORMAL_SESSION_ID, 2, PERMANENT_HASH)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action='complete_password_reset'",
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );
    insert_session(
        &database.pool,
        PASSWORD_SESSION_ID,
        USER_ID,
        "password_change",
        None,
    )
    .await;
    insert_session(
        &database.pool,
        OTHER_PASSWORD_SESSION_ID,
        USER_ID,
        "password_change",
        None,
    )
    .await;

    let barrier = Arc::new(Barrier::new(3));
    let first_repository = repository.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_repository
            .complete_temporary_password(
                USER_ID,
                PASSWORD_SESSION_ID,
                2,
                "$argon2id$completion-winner-a",
            )
            .await
    });
    let second_repository = repository.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_repository
            .complete_temporary_password(
                USER_ID,
                OTHER_PASSWORD_SESSION_ID,
                2,
                "$argon2id$completion-winner-b",
            )
            .await
    });
    barrier.wait().await;
    let outcomes = [
        first.await.unwrap().unwrap(),
        second.await.unwrap().unwrap(),
    ];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_some()).count(),
        1
    );
    let winner = outcomes.into_iter().flatten().next().unwrap();
    assert_eq!(winner.id, USER_ID);
    assert_eq!(winner.auth_version, 3);
    assert_eq!(winner.session_purpose, ConsoleSessionPurpose::Normal);
    assert!(winner.temporary_password_expires_at.is_none());

    let state = sqlx::query_as::<_, (String, i64, bool, Option<String>, Option<String>)>(
        "SELECT password_hash,auth_version,password_change_required, \
                temporary_password_issued_at,temporary_password_expires_at \
         FROM users WHERE id=?1",
    )
    .bind(USER_ID)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(
        [
            "$argon2id$completion-winner-a",
            "$argon2id$completion-winner-b"
        ]
        .contains(&state.0.as_str())
    );
    assert_eq!(state.1, 3);
    assert!(!state.2);
    assert!(state.3.is_none());
    assert!(state.4.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM user_sessions WHERE user_id=?1 AND revoked_at IS NULL",
        )
        .bind(USER_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );
    let completion_audit = sqlx::query_as::<_, (String, String)>(
        "SELECT before_redacted,after_redacted FROM audit_logs \
         WHERE action='complete_password_reset'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action='complete_password_reset'",
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        1
    );
    let combined = format!("{}{}", completion_audit.0, completion_audit.1);
    assert!(!combined.contains("completion-winner"));
    assert!(!combined.contains(TEMPORARY_HASH));
    assert!(!combined.contains("password_hash"));
}

#[tokio::test]
async fn sqlite_account_repository_serializes_bootstrap_and_resets_admins() {
    let database = migrated_pool().await;
    let repository = SqliteAuthRepository::new(database.pool.clone());
    assert!(matches!(
        repository.bootstrap_admin(" ", " ", "").await,
        Err(RepositoryError::Validation)
    ));

    let barrier = Arc::new(Barrier::new(3));
    let first_repository = repository.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_repository
            .bootstrap_admin("  ÄDMIN-A@EXAMPLE.TEST  ", "SQLite bootstrap A", OLD_HASH)
            .await
    });
    let second_repository = repository.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_repository
            .bootstrap_admin("  ÜDMIN-B@EXAMPLE.TEST  ", "SQLite bootstrap B", OLD_HASH)
            .await
    });
    barrier.wait().await;
    let results = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(RepositoryError::Conflict)))
            .count(),
        1
    );
    let admin_id = results.into_iter().find_map(Result::ok).unwrap();
    let admin = sqlx::query_as::<_, (String, String, Uuid, i64, Option<DateTime<Utc>>)>(
        "SELECT email,display_name,user_group_id,auth_version,password_changed_at \
         FROM users WHERE id=?1",
    )
    .bind(admin_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(["ädmin-a@example.test", "üdmin-b@example.test"].contains(&admin.0.as_str()));
    assert!(["SQLite bootstrap A", "SQLite bootstrap B"].contains(&admin.1.as_str()));
    assert_eq!(admin.2, DEFAULT_ADMIN_GROUP_ID);
    assert_eq!(admin.3, 1);
    assert!(admin.4.is_some());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_logs WHERE action='bootstrap'",)
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        repository.bootstrap_admin("", "", "").await,
        Err(RepositoryError::Conflict)
    ));

    insert_session(&database.pool, SESSION_ID, admin_id, "normal", None).await;
    assert!(
        repository
            .reset_active_admin_password(&admin.0.to_uppercase(), PERMANENT_HASH)
            .await
            .unwrap()
    );
    let reset = sqlx::query_as::<_, (String, i64, bool, Option<String>, Option<String>)>(
        "SELECT password_hash,auth_version,password_change_required, \
                temporary_password_issued_at,temporary_password_expires_at \
         FROM users WHERE id=?1",
    )
    .bind(admin_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(reset.0, PERMANENT_HASH);
    assert_eq!(reset.1, 2);
    assert!(!reset.2);
    assert!(reset.3.is_none());
    assert!(reset.4.is_none());
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1",
        )
        .bind(SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_some()
    );
    let reset_audit = sqlx::query_as::<_, (Option<Uuid>, String, String, String)>(
        "SELECT actor_user_id,actor_type,before_redacted,after_redacted \
         FROM audit_logs WHERE action='reset_password'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(reset_audit.0.is_none());
    assert_eq!(reset_audit.1, "system");
    assert_eq!(
        serde_json::from_str::<Value>(&reset_audit.2).unwrap()["email"],
        admin.0
    );
    assert_eq!(
        serde_json::from_str::<Value>(&reset_audit.3).unwrap()["password_changed"],
        true
    );
    assert!(!reset_audit.3.contains(PERMANENT_HASH));

    insert_user(
        &database.pool,
        OTHER_USER_ID,
        "not-admin@example.test",
        "SQLite non-admin",
        "user",
        "active",
        Some(OLD_HASH),
        "0",
    )
    .await;
    assert!(
        !repository
            .reset_active_admin_password("NOT-ADMIN@EXAMPLE.TEST", PERMANENT_HASH)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn sqlite_account_repository_rolls_back_when_audit_insertion_fails() {
    let database = migrated_pool().await;
    let repository = SqliteAuthRepository::new(database.pool.clone());
    sqlx::query(
        "CREATE TRIGGER force_audit_failure \
         BEFORE INSERT ON audit_logs BEGIN \
             SELECT RAISE(ABORT, 'forced audit failure'); \
         END",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(matches!(
        repository
            .bootstrap_admin(
                "rollback-admin@example.test",
                "SQLite rollback admin",
                OLD_HASH,
            )
            .await,
        Err(RepositoryError::Sql(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER force_audit_failure")
        .execute(&database.pool)
        .await
        .unwrap();
    let admin_id = repository
        .bootstrap_admin(
            "rollback-admin@example.test",
            "SQLite rollback admin",
            OLD_HASH,
        )
        .await
        .unwrap();
    insert_user(
        &database.pool,
        USER_ID,
        "rollback-user@example.test",
        "SQLite rollback user",
        "user",
        "active",
        Some(OLD_HASH),
        "0",
    )
    .await;
    insert_session(&database.pool, SESSION_ID, USER_ID, "normal", None).await;
    insert_session(&database.pool, OTHER_SESSION_ID, admin_id, "normal", None).await;
    sqlx::query(
        "CREATE TRIGGER force_audit_failure \
         BEFORE INSERT ON audit_logs BEGIN \
             SELECT RAISE(ABORT, 'forced audit failure'); \
         END",
    )
    .execute(&database.pool)
    .await
    .unwrap();

    assert!(matches!(
        repository
            .issue_temporary_password(
                admin_id,
                1,
                USER_ID,
                TEMPORARY_HASH,
                Duration::from_secs(60),
            )
            .await,
        Err(RepositoryError::Sql(_))
    ));
    assert_eq!(
        sqlx::query_as::<_, (String, i64, bool)>(
            "SELECT password_hash,auth_version,password_change_required FROM users WHERE id=?1",
        )
        .bind(USER_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        (OLD_HASH.to_owned(), 1, false)
    );
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1",
        )
        .bind(SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_none()
    );

    assert!(matches!(
        repository
            .reset_active_admin_password("ROLLBACK-ADMIN@EXAMPLE.TEST", PERMANENT_HASH)
            .await,
        Err(RepositoryError::Sql(_))
    ));
    assert_eq!(
        sqlx::query_as::<_, (String, i64)>(
            "SELECT password_hash,auth_version FROM users WHERE id=?1",
        )
        .bind(admin_id)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        (OLD_HASH.to_owned(), 1)
    );
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1",
        )
        .bind(OTHER_SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_none()
    );

    sqlx::query("DROP TRIGGER force_audit_failure")
        .execute(&database.pool)
        .await
        .unwrap();
    repository
        .issue_temporary_password(
            admin_id,
            1,
            USER_ID,
            TEMPORARY_HASH,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    insert_session(
        &database.pool,
        PASSWORD_SESSION_ID,
        USER_ID,
        "password_change",
        None,
    )
    .await;
    sqlx::query(
        "CREATE TRIGGER force_audit_failure \
         BEFORE INSERT ON audit_logs BEGIN \
             SELECT RAISE(ABORT, 'forced audit failure'); \
         END",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(matches!(
        repository
            .complete_temporary_password(USER_ID, PASSWORD_SESSION_ID, 2, PERMANENT_HASH)
            .await,
        Err(RepositoryError::Sql(_))
    ));
    assert_eq!(
        sqlx::query_as::<_, (String, i64, bool)>(
            "SELECT password_hash,auth_version,password_change_required FROM users WHERE id=?1",
        )
        .bind(USER_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        (TEMPORARY_HASH.to_owned(), 2, true)
    );
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1",
        )
        .bind(PASSWORD_SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_none()
    );
}
