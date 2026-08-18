#![cfg(feature = "sqlite-backend")]

use std::{sync::Arc, time::Duration};

use ai_gateway::{
    domain::ConsoleSessionPurpose,
    persistence::{
        ConsoleSessionState, DEFAULT_USER_GROUP_ID, RepositoryError, SQLITE_MIGRATOR,
        SessionRotation, SqliteAuthRepository,
    },
};
use chrono::{DateTime, Utc};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tempfile::TempDir;
use tokio::sync::Barrier;
use uuid::Uuid;

const USER_ID: Uuid = Uuid::from_u128(0x501);
const OTHER_USER_ID: Uuid = Uuid::from_u128(0x502);
const SESSION_ID: Uuid = Uuid::from_u128(0x511);
const OTHER_SESSION_ID: Uuid = Uuid::from_u128(0x512);
const EXPIRED_SESSION_ID: Uuid = Uuid::from_u128(0x513);
const REVOKED_SESSION_ID: Uuid = Uuid::from_u128(0x514);
const PASSWORD_SESSION_ID: Uuid = Uuid::from_u128(0x515);

const FAR_FUTURE: &str = "9999-01-02T03:04:05.678Z";
const TEMPORARY_PASSWORD_EXPIRY: &str = "9998-02-03T04:05:06.789Z";
const OLD_HASH: &[u8] = b"old-refresh-hash";
const NEXT_HASH: &[u8] = b"next-refresh-hash";

struct TestDatabase {
    _directory: TempDir,
    pool: SqlitePool,
}

async fn migrated_pool() -> TestDatabase {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("console-auth.sqlite3");
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

async fn insert_user(pool: &SqlitePool, id: Uuid, email: &str, display_name: &str, status: &str) {
    sqlx::query(
        "INSERT INTO users \
         (id,email,display_name,role,status,password_hash,user_group_id) \
         VALUES (?1,?2,?3,'user',?4,?5,?6)",
    )
    .bind(id)
    .bind(email)
    .bind(display_name)
    .bind(status)
    .bind("$argon2id$test-password-hash")
    .bind(DEFAULT_USER_GROUP_ID)
    .execute(pool)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_session(
    pool: &SqlitePool,
    id: Uuid,
    user_id: Uuid,
    hash: &[u8],
    created_at: &str,
    expires_at: &str,
    revoked_at: Option<&str>,
    user_agent: Option<&str>,
    purpose: &str,
) {
    sqlx::query(
        "INSERT INTO user_sessions \
         (id,user_id,refresh_token_hash,created_at,last_seen_at,expires_at,revoked_at, \
          user_agent,purpose) \
         VALUES (?1,?2,?3,?4,?4,?5,?6,?7,?8)",
    )
    .bind(id)
    .bind(user_id)
    .bind(hash)
    .bind(created_at)
    .bind(expires_at)
    .bind(revoked_at)
    .bind(user_agent)
    .bind(purpose)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn sqlite_auth_repository_loads_login_and_validates_session_purpose() {
    let database = migrated_pool().await;
    insert_user(
        &database.pool,
        USER_ID,
        "user@example.test",
        "SQLite auth user",
        "active",
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    let login = repository
        .find_login_user("user@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(login.id, USER_ID);
    assert_eq!(login.email.as_deref(), Some("user@example.test"));
    assert_eq!(login.display_name, "SQLite auth user");
    assert_eq!(login.role, "user");
    assert_eq!(login.status, "active");
    assert_eq!(
        login.password_hash.as_deref(),
        Some("$argon2id$test-password-hash")
    );
    assert_eq!(login.auth_version, 1);
    assert!(!login.password_change_required);
    assert!(login.temporary_password_expires_at.is_none());
    assert!(
        repository
            .find_login_user("missing@example.test")
            .await
            .unwrap()
            .is_none()
    );
    insert_user(
        &database.pool,
        OTHER_USER_ID,
        "üser@example.test",
        "SQLite Unicode auth user",
        "active",
    )
    .await;
    assert_eq!(
        repository
            .find_login_user("ÜSER@EXAMPLE.TEST")
            .await
            .unwrap()
            .unwrap()
            .id,
        OTHER_USER_ID
    );

    let password_user = repository.password_user(USER_ID).await.unwrap().unwrap();
    assert_eq!(password_user.id, USER_ID);
    assert_eq!(password_user.role, "user");
    assert_eq!(password_user.auth_version, 1);

    repository
        .create_session(
            SESSION_ID,
            USER_ID,
            OLD_HASH,
            timestamp(FAR_FUTURE),
            Some("SQLite test agent"),
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();
    let identity = repository
        .validate_console_identity(USER_ID, SESSION_ID, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(identity.user_id, USER_ID);
    assert_eq!(identity.session_id, SESSION_ID);
    assert_eq!(identity.session_purpose, "normal");
    assert!(
        repository
            .validate_console_identity(USER_ID, SESSION_ID, 2)
            .await
            .unwrap()
            .is_none()
    );

    sqlx::query("UPDATE user_sessions SET revoked_at=?2 WHERE id=?1")
        .bind(SESSION_ID)
        .bind("9998-01-01T00:00:00.000Z")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        repository
            .validate_console_identity(USER_ID, SESSION_ID, 1)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE user_sessions SET revoked_at=NULL WHERE id=?1")
        .bind(SESSION_ID)
        .execute(&database.pool)
        .await
        .unwrap();
    insert_session(
        &database.pool,
        EXPIRED_SESSION_ID,
        USER_ID,
        b"expired",
        "1999-01-01T00:00:00.000Z",
        "2000-01-01T00:00:00.000Z",
        None,
        None,
        "normal",
    )
    .await;
    assert!(
        repository
            .validate_console_identity(USER_ID, EXPIRED_SESSION_ID, 1)
            .await
            .unwrap()
            .is_none()
    );

    sqlx::query(
        "UPDATE users SET password_change_required=1,temporary_password_issued_at=?2, \
         temporary_password_expires_at=?3 WHERE id=?1",
    )
    .bind(USER_ID)
    .bind("9998-01-01T00:00:00.000Z")
    .bind(TEMPORARY_PASSWORD_EXPIRY)
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(
        repository
            .validate_console_identity(USER_ID, SESSION_ID, 1)
            .await
            .unwrap()
            .is_none()
    );

    repository
        .create_session(
            PASSWORD_SESSION_ID,
            USER_ID,
            OLD_HASH,
            timestamp(FAR_FUTURE),
            None,
            ConsoleSessionPurpose::PasswordChange,
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .validate_console_identity(USER_ID, PASSWORD_SESSION_ID, 1)
            .await
            .unwrap()
            .unwrap()
            .session_purpose,
        "password_change"
    );

    sqlx::query(
        "UPDATE users SET temporary_password_issued_at=?2,temporary_password_expires_at=?3 \
         WHERE id=?1",
    )
    .bind(USER_ID)
    .bind("1999-01-01T00:00:00.000Z")
    .bind("2000-01-01T00:00:00.000Z")
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(
        repository
            .validate_console_identity(USER_ID, PASSWORD_SESSION_ID, 1)
            .await
            .unwrap()
            .is_none()
    );

    sqlx::query("UPDATE users SET status='suspended' WHERE id=?1")
        .bind(USER_ID)
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(
        repository
            .validate_console_identity(USER_ID, PASSWORD_SESSION_ID, 1)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn sqlite_auth_repository_rotates_replays_and_caps_password_sessions() {
    let database = migrated_pool().await;
    insert_user(
        &database.pool,
        USER_ID,
        "rotate@example.test",
        "SQLite rotate user",
        "active",
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());
    repository
        .create_session(
            SESSION_ID,
            USER_ID,
            OLD_HASH,
            timestamp(FAR_FUTURE),
            Some("old agent"),
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();

    let requested_expiry = timestamp("9998-03-04T05:06:07.123456Z");
    let rotation = repository
        .rotate_session(
            SESSION_ID,
            OLD_HASH,
            NEXT_HASH,
            requested_expiry,
            Some("new agent"),
        )
        .await
        .unwrap();
    let SessionRotation::Rotated {
        user,
        refresh_expires_at,
    } = rotation
    else {
        panic!("the live session should rotate");
    };
    assert_eq!(user.id, USER_ID);
    assert_eq!(user.auth_version, 1);
    assert_eq!(user.session_purpose, ConsoleSessionPurpose::Normal);
    assert_eq!(refresh_expires_at, requested_expiry);

    let stored = sqlx::query_as::<
        _,
        (
            Vec<u8>,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
            Option<String>,
            Option<DateTime<Utc>>,
        ),
    >(
        "SELECT refresh_token_hash,expires_at,rotated_at,last_seen_at,user_agent,revoked_at \
         FROM user_sessions WHERE id=?1",
    )
    .bind(SESSION_ID)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(stored.0, NEXT_HASH);
    assert_eq!(stored.1, requested_expiry);
    assert!(stored.2.is_some());
    assert!(stored.3.is_some());
    assert_eq!(stored.4.as_deref(), Some("new agent"));
    assert!(stored.5.is_none());

    assert!(matches!(
        repository
            .rotate_session(SESSION_ID, OLD_HASH, b"unused", timestamp(FAR_FUTURE), None,)
            .await
            .unwrap(),
        SessionRotation::Replayed
    ));
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
    assert!(matches!(
        repository
            .rotate_session(
                SESSION_ID,
                NEXT_HASH,
                b"unused",
                timestamp(FAR_FUTURE),
                None,
            )
            .await
            .unwrap(),
        SessionRotation::Invalid
    ));

    sqlx::query(
        "UPDATE users SET password_change_required=1,temporary_password_issued_at=?2, \
         temporary_password_expires_at=?3 WHERE id=?1",
    )
    .bind(USER_ID)
    .bind("9998-01-01T00:00:00.000Z")
    .bind(TEMPORARY_PASSWORD_EXPIRY)
    .execute(&database.pool)
    .await
    .unwrap();
    repository
        .create_session(
            PASSWORD_SESSION_ID,
            USER_ID,
            OLD_HASH,
            timestamp(FAR_FUTURE),
            None,
            ConsoleSessionPurpose::PasswordChange,
        )
        .await
        .unwrap();
    let SessionRotation::Rotated {
        refresh_expires_at, ..
    } = repository
        .rotate_session(
            PASSWORD_SESSION_ID,
            OLD_HASH,
            NEXT_HASH,
            timestamp(FAR_FUTURE),
            None,
        )
        .await
        .unwrap()
    else {
        panic!("the password-change session should rotate");
    };
    assert_eq!(refresh_expires_at, timestamp(TEMPORARY_PASSWORD_EXPIRY));
    assert_eq!(
        sqlx::query_scalar::<_, DateTime<Utc>>("SELECT expires_at FROM user_sessions WHERE id=?1",)
            .bind(PASSWORD_SESSION_ID)
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        timestamp(TEMPORARY_PASSWORD_EXPIRY)
    );

    sqlx::query("UPDATE users SET status='disabled' WHERE id=?1")
        .bind(USER_ID)
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(matches!(
        repository
            .rotate_session(
                PASSWORD_SESSION_ID,
                NEXT_HASH,
                b"disabled-next",
                timestamp(FAR_FUTURE),
                None,
            )
            .await
            .unwrap(),
        SessionRotation::Invalid
    ));
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

#[tokio::test]
async fn sqlite_auth_repository_serializes_concurrent_refresh_replay() {
    let database = migrated_pool().await;
    insert_user(
        &database.pool,
        USER_ID,
        "concurrent@example.test",
        "SQLite concurrent user",
        "active",
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());
    repository
        .create_session(
            SESSION_ID,
            USER_ID,
            OLD_HASH,
            timestamp(FAR_FUTURE),
            None,
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();

    let barrier = Arc::new(Barrier::new(3));
    let first_repository = repository.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_repository
            .rotate_session(
                SESSION_ID,
                OLD_HASH,
                NEXT_HASH,
                timestamp(FAR_FUTURE),
                Some("first"),
            )
            .await
    });
    let second_repository = repository.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_repository
            .rotate_session(
                SESSION_ID,
                OLD_HASH,
                NEXT_HASH,
                timestamp(FAR_FUTURE),
                Some("second"),
            )
            .await
    });
    barrier.wait().await;
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    let outcomes = [&first, &second];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SessionRotation::Rotated { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SessionRotation::Replayed))
            .count(),
        1
    );
    let stored = sqlx::query_as::<_, (Vec<u8>, Option<DateTime<Utc>>)>(
        "SELECT refresh_token_hash,revoked_at FROM user_sessions WHERE id=?1",
    )
    .bind(SESSION_ID)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(stored.0, NEXT_HASH);
    assert!(stored.1.is_some());
}

#[tokio::test]
async fn sqlite_auth_repository_lists_and_revokes_owned_sessions() {
    let database = migrated_pool().await;
    insert_user(
        &database.pool,
        USER_ID,
        "sessions@example.test",
        "SQLite sessions user",
        "active",
    )
    .await;
    insert_user(
        &database.pool,
        OTHER_USER_ID,
        "other@example.test",
        "SQLite other user",
        "active",
    )
    .await;
    insert_session(
        &database.pool,
        SESSION_ID,
        USER_ID,
        b"current",
        "2020-01-01T00:00:00.000Z",
        FAR_FUTURE,
        None,
        Some("current"),
        "normal",
    )
    .await;
    insert_session(
        &database.pool,
        OTHER_SESSION_ID,
        USER_ID,
        b"other",
        "2025-01-01T00:00:00.000Z",
        FAR_FUTURE,
        None,
        Some("other"),
        "normal",
    )
    .await;
    insert_session(
        &database.pool,
        EXPIRED_SESSION_ID,
        USER_ID,
        b"expired",
        "1999-01-01T00:00:00.000Z",
        "2000-01-01T00:00:00.000Z",
        None,
        None,
        "normal",
    )
    .await;
    insert_session(
        &database.pool,
        REVOKED_SESSION_ID,
        USER_ID,
        b"revoked",
        "2024-01-01T00:00:00.000Z",
        FAR_FUTURE,
        Some("2025-01-01T00:00:00.000Z"),
        None,
        "normal",
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    let sessions = repository
        .sessions_for_user(USER_ID, SESSION_ID)
        .await
        .unwrap();
    assert_eq!(
        sessions
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>(),
        [
            SESSION_ID,
            OTHER_SESSION_ID,
            REVOKED_SESSION_ID,
            EXPIRED_SESSION_ID
        ]
    );
    assert!(sessions[0].is_current);
    assert!(matches!(sessions[0].state, ConsoleSessionState::Active));
    assert!(matches!(sessions[1].state, ConsoleSessionState::Active));
    assert!(matches!(sessions[2].state, ConsoleSessionState::Revoked));
    assert!(matches!(sessions[3].state, ConsoleSessionState::Expired));

    assert!(
        !repository
            .revoke_session_for_user(OTHER_USER_ID, SESSION_ID)
            .await
            .unwrap()
    );
    assert_eq!(
        repository
            .revoke_other_sessions(USER_ID, SESSION_ID)
            .await
            .unwrap(),
        1
    );
    assert!(
        repository
            .revoke_session_for_user(USER_ID, SESSION_ID)
            .await
            .unwrap()
    );
    repository.revoke_all_sessions(USER_ID).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM user_sessions \
             WHERE user_id=?1 AND revoked_at IS NULL",
        )
        .bind(USER_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn sqlite_auth_repository_fails_closed_on_malformed_timestamps() {
    let database = migrated_pool().await;
    insert_user(
        &database.pool,
        USER_ID,
        "malformed@example.test",
        "SQLite malformed user",
        "active",
    )
    .await;
    insert_session(
        &database.pool,
        SESSION_ID,
        USER_ID,
        OLD_HASH,
        "2000-01-01T00:00:00.000Z",
        "not-a-timestamp",
        None,
        None,
        "normal",
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool);

    let error = repository
        .sessions_for_user(USER_ID, SESSION_ID)
        .await
        .unwrap_err();
    assert!(matches!(error, RepositoryError::Corrupt(_)));
}
