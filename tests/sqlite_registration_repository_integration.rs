#![cfg(feature = "sqlite-backend")]

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use ai_gateway::{
    domain::{ConsoleSessionPurpose, UserRole},
    persistence::{
        DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID, InviteUserInput, RegistrationAttempt,
        RegistrationInvitationCodeInput, RepositoryError, SQLITE_MIGRATOR, SqliteAuthRepository,
        SqliteDecimal,
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

const ADMIN_ID: Uuid = Uuid::from_u128(0x701);
const INACTIVE_ADMIN_ID: Uuid = Uuid::from_u128(0x702);
const EXISTING_USER_ID: Uuid = Uuid::from_u128(0x703);
const ELIGIBILITY_USER_ID: Uuid = Uuid::from_u128(0x704);
const EMAILLESS_USER_ID: Uuid = Uuid::from_u128(0x705);
const SYSTEM_USER_ID: Uuid = Uuid::from_u128(0x706);
const MISSING_ID: Uuid = Uuid::from_u128(0x7ff);

const CODE_ID: Uuid = Uuid::from_u128(0x711);
const DISABLED_CODE_ID: Uuid = Uuid::from_u128(0x712);
const EXPIRED_CODE_ID: Uuid = Uuid::from_u128(0x713);
const EXHAUSTED_CODE_ID: Uuid = Uuid::from_u128(0x714);
const ROLLBACK_CODE_ID: Uuid = Uuid::from_u128(0x715);
const RACE_CODE_ID: Uuid = Uuid::from_u128(0x716);

const INVITATION_ID: Uuid = Uuid::from_u128(0x721);
const CONCURRENT_INVITATION_ID: Uuid = Uuid::from_u128(0x722);
const REPLACEMENT_INVITATION_ID: Uuid = Uuid::from_u128(0x723);
const ROLLBACK_INVITATION_ID: Uuid = Uuid::from_u128(0x724);
const ROLLBACK_REPLACEMENT_ID: Uuid = Uuid::from_u128(0x725);
const CONCURRENT_REISSUE_A_ID: Uuid = Uuid::from_u128(0x726);
const CONCURRENT_REISSUE_B_ID: Uuid = Uuid::from_u128(0x727);
const SESSION_ID: Uuid = Uuid::from_u128(0x731);
const ROLLBACK_SESSION_ID: Uuid = Uuid::from_u128(0x732);
const ENABLED_POLICY_ID: Uuid = Uuid::from_u128(0x741);
const DISABLED_POLICY_ID: Uuid = Uuid::from_u128(0x742);

const FAR_FUTURE: &str = "9999-01-02T03:04:05.678Z";
const EXPIRED: &str = "2000-01-02T03:04:05.678Z";
const EXACT_BALANCE: &str = "1234567890123456.12345678";
const PASSWORD_HASH: &str = "$argon2id$registration-password";

struct TestDatabase {
    _directory: TempDir,
    pool: SqlitePool,
}

async fn migrated_pool() -> TestDatabase {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("console-registration.sqlite3");
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(6)
        .connect_with(options)
        .await
        .unwrap();
    SQLITE_MIGRATOR.run(&pool).await.unwrap();
    TestDatabase {
        _directory: directory,
        pool,
    }
}

fn decimal(value: &str) -> Decimal {
    Decimal::from_str_exact(value).unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn insert_user(
    pool: &SqlitePool,
    id: Uuid,
    email: Option<&str>,
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
    .bind(SqliteDecimal::from(decimal(balance)))
    .bind(group_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_admin(pool: &SqlitePool) {
    insert_user(
        pool,
        ADMIN_ID,
        Some("admin@example.test"),
        "SQLite registration administrator",
        "admin",
        "active",
        Some("$argon2id$admin-password"),
        "0",
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn insert_code(
    pool: &SqlitePool,
    id: Uuid,
    code_hash: &[u8],
    name: &str,
    max_uses: Option<i64>,
    used_count: i64,
    expires_at: Option<&str>,
    enabled: bool,
    balance: &str,
) {
    sqlx::query(
        "INSERT INTO registration_invitation_codes \
         (id,name,code_hash,max_uses,used_count,expires_at,enabled,user_group_id, \
          initial_balance_amount,created_by) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
    )
    .bind(id)
    .bind(name)
    .bind(code_hash)
    .bind(max_uses)
    .bind(used_count)
    .bind(expires_at)
    .bind(enabled)
    .bind(DEFAULT_USER_GROUP_ID)
    .bind(SqliteDecimal::from(decimal(balance)))
    .bind(ADMIN_ID)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_session(pool: &SqlitePool, id: Uuid, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO user_sessions \
         (id,user_id,refresh_token_hash,created_at,last_seen_at,expires_at,purpose) \
         VALUES (?1,?2,?3,'2026-01-01T00:00:00.000Z', \
                 '2026-01-01T00:00:00.000Z',?4,'normal')",
    )
    .bind(id)
    .bind(user_id)
    .bind(format!("refresh-{id}").into_bytes())
    .bind(FAR_FUTURE)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_policy(pool: &SqlitePool, id: Uuid, name: &str, enabled: bool) {
    sqlx::query("INSERT INTO api_key_policies (id,name,enabled) VALUES (?1,?2,?3)")
        .bind(id)
        .bind(name)
        .bind(enabled)
        .execute(pool)
        .await
        .unwrap();
}

fn code_input(
    name: &str,
    max_uses: Option<i64>,
    enabled: bool,
    group_id: Uuid,
    balance: &str,
) -> RegistrationInvitationCodeInput {
    RegistrationInvitationCodeInput {
        name: name.to_owned(),
        max_uses,
        expires_at: Some(
            DateTime::parse_from_rfc3339(FAR_FUTURE)
                .unwrap()
                .with_timezone(&Utc),
        ),
        enabled,
        user_group_id: group_id,
        initial_balance_amount: decimal(balance),
    }
}

fn invite_input(
    email: &str,
    display_name: &str,
    group_id: Option<Uuid>,
    policy_id: Option<Uuid>,
) -> InviteUserInput {
    InviteUserInput {
        email: email.to_owned(),
        display_name: display_name.to_owned(),
        role: UserRole::User,
        initial_balance_amount: decimal(EXACT_BALANCE),
        user_group_id: group_id,
        default_api_key_policy_id: policy_id,
    }
}

fn assert_object_keys(value: &Value, expected: &[&str]) {
    let actual = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}

async fn install_audit_failure(pool: &SqlitePool) {
    sqlx::query(
        "CREATE TRIGGER force_audit_failure \
         BEFORE INSERT ON audit_logs BEGIN \
             SELECT RAISE(ABORT, 'forced audit failure'); \
         END",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn remove_audit_failure(pool: &SqlitePool) {
    sqlx::query("DROP TRIGGER force_audit_failure")
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn sqlite_registration_codes_validate_conflicts_etags_and_redacted_audits() {
    let database = migrated_pool().await;
    insert_admin(&database.pool).await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    for invalid in [
        code_input(" ", Some(3), true, DEFAULT_USER_GROUP_ID, "0"),
        code_input("too few uses", Some(0), true, DEFAULT_USER_GROUP_ID, "0"),
        code_input(
            "negative balance",
            Some(3),
            true,
            DEFAULT_USER_GROUP_ID,
            "-0.01",
        ),
        code_input("missing group", Some(3), true, MISSING_ID, "0"),
    ] {
        assert!(matches!(
            repository
                .create_registration_invitation_code(ADMIN_ID, b"invalid-code", invalid)
                .await,
            Err(RepositoryError::Validation)
        ));
    }

    let created = repository
        .create_registration_invitation_code(
            ADMIN_ID,
            b"alpha-code-hash",
            code_input(
                "  Alpha code  ",
                Some(3),
                true,
                DEFAULT_USER_GROUP_ID,
                EXACT_BALANCE,
            ),
        )
        .await
        .unwrap();
    let beta = repository
        .create_registration_invitation_code(
            ADMIN_ID,
            b"beta-code-hash",
            code_input("Beta code", None, true, DEFAULT_USER_GROUP_ID, "0"),
        )
        .await
        .unwrap();

    assert!(matches!(
        repository
            .create_registration_invitation_code(
                ADMIN_ID,
                b"different-code-hash",
                code_input("Alpha code", Some(1), true, DEFAULT_USER_GROUP_ID, "0"),
            )
            .await,
        Err(RepositoryError::RegistrationInvitationCodeConflict)
    ));
    assert!(matches!(
        repository
            .create_registration_invitation_code(
                ADMIN_ID,
                b"alpha-code-hash",
                code_input("Different name", Some(1), true, DEFAULT_USER_GROUP_ID, "0"),
            )
            .await,
        Err(RepositoryError::RegistrationInvitationCodeConflict)
    ));

    let fetched = repository
        .registration_invitation_code(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.name, "Alpha code");
    assert_eq!(fetched.max_uses, Some(3));
    assert_eq!(fetched.used_count, 0);
    assert!(fetched.expires_at.is_some());
    assert!(fetched.enabled);
    assert_eq!(fetched.user_group_id, DEFAULT_USER_GROUP_ID);
    assert_eq!(fetched.initial_balance_amount, decimal(EXACT_BALANCE));
    assert_eq!(fetched.created_by, ADMIN_ID);
    assert!(fetched.last_used_at.is_none());
    assert!(
        repository
            .registration_invitation_code(MISSING_ID)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                MISSING_ID,
                code_input("Missing code", Some(1), true, DEFAULT_USER_GROUP_ID, "0",),
                Utc::now(),
            )
            .await,
        Err(RepositoryError::NotFound)
    ));
    let listed = repository.registration_invitation_codes().await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|code| code.id == created.id));
    assert!(listed.iter().any(|code| code.id == beta.id));

    let rounded = repository
        .create_registration_invitation_code(
            ADMIN_ID,
            b"rounded-code-hash",
            code_input(
                "Rounded code",
                Some(1),
                true,
                DEFAULT_USER_GROUP_ID,
                "0.000000005",
            ),
        )
        .await
        .unwrap();
    let rounded_before = repository
        .registration_invitation_code(rounded.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rounded_before.initial_balance_amount, decimal("0.00000001"));
    repository
        .update_registration_invitation_code(
            ADMIN_ID,
            rounded.id,
            code_input(
                "Rounded code updated",
                Some(1),
                true,
                DEFAULT_USER_GROUP_ID,
                "1.234567895",
            ),
            rounded_before.updated_at,
        )
        .await
        .unwrap();
    let rounded_after = repository
        .registration_invitation_code(rounded.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rounded_after.initial_balance_amount, decimal("1.23456790"));
    assert!(rounded_after.updated_at > rounded_before.updated_at);

    sqlx::query("UPDATE registration_invitation_codes SET used_count=2 WHERE id=?1")
        .bind(created.id)
        .execute(&database.pool)
        .await
        .unwrap();
    let before = repository
        .registration_invitation_code(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                created.id,
                code_input(
                    "Alpha code",
                    Some(1),
                    true,
                    DEFAULT_USER_GROUP_ID,
                    EXACT_BALANCE,
                ),
                before.updated_at,
            )
            .await,
        Err(RepositoryError::Validation)
    ));

    let first_update = repository
        .update_registration_invitation_code(
            ADMIN_ID,
            created.id,
            code_input(
                "Alpha updated",
                Some(4),
                true,
                DEFAULT_USER_GROUP_ID,
                EXACT_BALANCE,
            ),
            before.updated_at,
        )
        .await
        .unwrap();
    let after_first = repository
        .registration_invitation_code(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(after_first.updated_at > before.updated_at);
    assert_eq!(after_first.name, "Alpha updated");
    assert_eq!(after_first.max_uses, Some(4));
    assert_eq!(after_first.used_count, 2);
    assert!(after_first.enabled);
    assert_eq!(after_first.initial_balance_amount, decimal(EXACT_BALANCE));
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                created.id,
                code_input(
                    "stale update",
                    Some(4),
                    true,
                    DEFAULT_USER_GROUP_ID,
                    EXACT_BALANCE,
                ),
                before.updated_at,
            )
            .await,
        Err(RepositoryError::Conflict)
    ));

    let second_update = repository
        .update_registration_invitation_code(
            ADMIN_ID,
            created.id,
            code_input(
                "Alpha updated again",
                Some(4),
                false,
                DEFAULT_USER_GROUP_ID,
                EXACT_BALANCE,
            ),
            after_first.updated_at,
        )
        .await
        .unwrap();
    let after_second = repository
        .registration_invitation_code(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(after_second.updated_at > after_first.updated_at);
    assert_eq!(after_second.name, "Alpha updated again");
    assert_eq!(after_second.max_uses, Some(4));
    assert_eq!(after_second.used_count, 2);
    assert!(!after_second.enabled);
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                created.id,
                code_input("Beta code", Some(4), true, DEFAULT_USER_GROUP_ID, "0"),
                after_second.updated_at,
            )
            .await,
        Err(RepositoryError::RegistrationInvitationCodeConflict)
    ));

    let audits = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT action,before_redacted,after_redacted,correlation_id \
         FROM audit_logs WHERE object_type='registration_invitation_code' AND object_id=?1",
    )
    .bind(created.id)
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert_eq!(audits.len(), 3);
    let expected_correlations = [
        created.correlation_id.to_string(),
        first_update.correlation_id.to_string(),
        second_update.correlation_id.to_string(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(
        audits
            .iter()
            .map(|audit| audit.3.clone().unwrap())
            .collect::<BTreeSet<_>>(),
        expected_correlations
    );
    let code_audit_keys = [
        "created_at",
        "created_by",
        "enabled",
        "expires_at",
        "id",
        "initial_balance_amount",
        "last_used_at",
        "max_uses",
        "name",
        "updated_at",
        "used_count",
        "user_group_id",
    ];
    for (action, before_redacted, after_redacted, _) in &audits {
        let before: Value = serde_json::from_str(before_redacted).unwrap();
        let after: Value = serde_json::from_str(after_redacted).unwrap();
        if action == "create" {
            assert_eq!(before, Value::Object(Default::default()));
        } else {
            assert_eq!(action, "update");
            assert_object_keys(&before, &code_audit_keys);
            assert!(before["initial_balance_amount"].is_string());
        }
        assert_object_keys(&after, &code_audit_keys);
        assert!(after["initial_balance_amount"].is_string());
        assert_eq!(
            after["initial_balance_amount"].as_str(),
            Some(EXACT_BALANCE)
        );
    }

    install_audit_failure(&database.pool).await;
    assert!(matches!(
        repository
            .create_registration_invitation_code(
                ADMIN_ID,
                b"audit-failure-code-hash",
                code_input(
                    "Audit failure create",
                    Some(1),
                    true,
                    DEFAULT_USER_GROUP_ID,
                    "1.25",
                ),
            )
            .await,
        Err(RepositoryError::Sql(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM registration_invitation_codes \
             WHERE name='Audit failure create'"
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                created.id,
                code_input(
                    "Audit failure update",
                    Some(4),
                    true,
                    DEFAULT_USER_GROUP_ID,
                    EXACT_BALANCE,
                ),
                after_second.updated_at,
            )
            .await,
        Err(RepositoryError::Sql(_))
    ));
    let after_failed_update = repository
        .registration_invitation_code(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(after_failed_update).unwrap(),
        serde_json::to_value(after_second).unwrap()
    );
    remove_audit_failure(&database.pool).await;
}

#[tokio::test]
async fn sqlite_code_registration_preserves_precedence_canonical_email_balance_and_atomicity() {
    let database = migrated_pool().await;
    insert_admin(&database.pool).await;
    insert_user(
        &database.pool,
        EXISTING_USER_ID,
        Some("duplicate@example.test"),
        "Existing duplicate",
        "user",
        "active",
        Some(PASSWORD_HASH),
        "0",
    )
    .await;
    insert_code(
        &database.pool,
        DISABLED_CODE_ID,
        b"disabled-code-hash",
        "Disabled",
        Some(2),
        0,
        Some(FAR_FUTURE),
        false,
        "0",
    )
    .await;
    insert_code(
        &database.pool,
        EXPIRED_CODE_ID,
        b"expired-code-hash",
        "Expired",
        Some(2),
        0,
        Some(EXPIRED),
        true,
        "0",
    )
    .await;
    insert_code(
        &database.pool,
        EXHAUSTED_CODE_ID,
        b"exhausted-code-hash",
        "Exhausted",
        Some(1),
        1,
        Some(FAR_FUTURE),
        true,
        "0",
    )
    .await;
    insert_code(
        &database.pool,
        CODE_ID,
        b"valid-code-hash",
        "Valid",
        Some(3),
        0,
        Some(FAR_FUTURE),
        true,
        EXACT_BALANCE,
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    for code_hash in [
        b"missing-code-hash".as_slice(),
        b"disabled-code-hash".as_slice(),
        b"expired-code-hash".as_slice(),
        b"exhausted-code-hash".as_slice(),
    ] {
        assert!(matches!(
            repository
                .register_with_invitation_code(
                    code_hash,
                    "DUPLICATE@EXAMPLE.TEST",
                    "Existing duplicate",
                    PASSWORD_HASH,
                )
                .await
                .unwrap(),
            RegistrationAttempt::InvalidCode
        ));
    }

    assert!(matches!(
        repository
            .register_with_invitation_code(
                b"valid-code-hash",
                " DUPLICATE@EXAMPLE.TEST ",
                "Unique duplicate attempt",
                PASSWORD_HASH,
            )
            .await
            .unwrap(),
        RegistrationAttempt::EmailConflict
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT used_count FROM registration_invitation_codes WHERE id=?1"
        )
        .bind(CODE_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );
    assert!(matches!(
        repository
            .register_with_invitation_code(
                b"valid-code-hash",
                "unique-email@example.test",
                "Existing duplicate",
                PASSWORD_HASH,
            )
            .await
            .unwrap(),
        RegistrationAttempt::EmailConflict
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT used_count FROM registration_invitation_codes WHERE id=?1"
        )
        .bind(CODE_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );

    let registration_etag = repository
        .registration_invitation_code(CODE_ID)
        .await
        .unwrap()
        .unwrap()
        .updated_at;
    let registered = match repository
        .register_with_invitation_code(
            b"valid-code-hash",
            "  TÉST@EXAMPLE.TEST  ",
            "Unicode registration",
            PASSWORD_HASH,
        )
        .await
        .unwrap()
    {
        RegistrationAttempt::Registered(user) => user,
        RegistrationAttempt::InvalidCode | RegistrationAttempt::EmailConflict => {
            panic!("valid registration did not succeed")
        }
    };
    assert_eq!(registered.email.as_deref(), Some("tést@example.test"));
    assert_eq!(registered.role, UserRole::User);
    assert_eq!(registered.session_purpose, ConsoleSessionPurpose::Normal);
    let state = sqlx::query_as::<_, (String, String, SqliteDecimal, Uuid, String)>(
        "SELECT email,status,balance_amount,user_group_id,password_hash FROM users WHERE id=?1",
    )
    .bind(registered.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(state.0, "tést@example.test");
    assert_eq!(state.1, "active");
    assert_eq!(state.2.into_inner(), decimal(EXACT_BALANCE));
    assert_eq!(state.3, DEFAULT_USER_GROUP_ID);
    assert_eq!(state.4, PASSWORD_HASH);
    let consumed_code = repository
        .registration_invitation_code(CODE_ID)
        .await
        .unwrap()
        .unwrap();
    assert!(consumed_code.updated_at > registration_etag);
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                CODE_ID,
                code_input(
                    "Stale after consumption",
                    Some(3),
                    true,
                    DEFAULT_USER_GROUP_ID,
                    EXACT_BALANCE,
                ),
                registration_etag,
            )
            .await,
        Err(RepositoryError::Conflict)
    ));

    let audit = sqlx::query_as::<_, (String, String)>(
        "SELECT before_redacted,after_redacted FROM audit_logs \
         WHERE action='register' AND object_id=?1",
    )
    .bind(registered.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&audit.0).unwrap(),
        Value::Object(Default::default())
    );
    let after: Value = serde_json::from_str(&audit.1).unwrap();
    assert_object_keys(
        &after,
        &[
            "balance_amount",
            "display_name",
            "email",
            "id",
            "registration_invitation_code_id",
            "role",
            "status",
            "user_group_id",
        ],
    );
    assert_eq!(after["balance_amount"].as_str(), Some(EXACT_BALANCE));

    insert_code(
        &database.pool,
        ROLLBACK_CODE_ID,
        b"rollback-code-hash",
        "Rollback",
        Some(1),
        0,
        Some(FAR_FUTURE),
        true,
        "1.00000001",
    )
    .await;
    install_audit_failure(&database.pool).await;
    assert!(matches!(
        repository
            .register_with_invitation_code(
                b"rollback-code-hash",
                "rollback@example.test",
                "Rollback registration",
                PASSWORD_HASH,
            )
            .await,
        Err(RepositoryError::Sql(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users WHERE email=?1")
            .bind("rollback@example.test")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT used_count FROM registration_invitation_codes WHERE id=?1"
        )
        .bind(ROLLBACK_CODE_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn sqlite_registration_code_final_use_is_consumed_once_under_concurrency() {
    let database = migrated_pool().await;
    insert_admin(&database.pool).await;
    insert_code(
        &database.pool,
        RACE_CODE_ID,
        b"last-use-code-hash",
        "Last use",
        Some(1),
        0,
        Some(FAR_FUTURE),
        true,
        "2.5",
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());
    let barrier = Arc::new(Barrier::new(3));

    let first_repository = repository.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_repository
            .register_with_invitation_code(
                b"last-use-code-hash",
                "race-a@example.test",
                "Registration race A",
                "$argon2id$race-a",
            )
            .await
            .unwrap()
    });
    let second_repository = repository.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_repository
            .register_with_invitation_code(
                b"last-use-code-hash",
                "race-b@example.test",
                "Registration race B",
                "$argon2id$race-b",
            )
            .await
            .unwrap()
    });
    barrier.wait().await;

    let outcomes = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RegistrationAttempt::Registered(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RegistrationAttempt::InvalidCode))
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT used_count FROM registration_invitation_codes WHERE id=?1"
        )
        .bind(RACE_CODE_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM users WHERE email IN ('race-a@example.test','race-b@example.test')"
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action='register' \
             AND object_id IN (SELECT id FROM users WHERE email LIKE 'race-%')"
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn sqlite_invitations_accept_valid_tokens_once_and_serialize_concurrent_acceptance() {
    let database = migrated_pool().await;
    insert_admin(&database.pool).await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    let created = repository
        .invite_user(
            ADMIN_ID,
            invite_input("  INVITED@EXAMPLE.TEST ", "Invited user", None, None),
            INVITATION_ID,
            b"invitation-token-hash",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    assert_eq!(created.invitation_id, INVITATION_ID);
    assert!(
        repository
            .accept_invitation(INVITATION_ID, b"wrong-token-hash", PASSWORD_HASH)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .accept_invitation(MISSING_ID, b"missing-token-hash", PASSWORD_HASH)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT accepted_at FROM user_invitations WHERE id=?1"
        )
        .bind(INVITATION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_none()
    );

    let accepted = repository
        .accept_invitation(INVITATION_ID, b"invitation-token-hash", PASSWORD_HASH)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.id, created.user_id);
    assert_eq!(accepted.email.as_deref(), Some("invited@example.test"));
    assert_eq!(accepted.auth_version, 2);
    let accepted_state = sqlx::query_as::<
        _,
        (
            String,
            i64,
            String,
            bool,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        ),
    >(
        "SELECT u.status,u.auth_version,u.password_hash,u.password_change_required, \
                i.accepted_at,i.revoked_at \
         FROM users AS u JOIN user_invitations AS i ON i.user_id=u.id WHERE i.id=?1",
    )
    .bind(INVITATION_ID)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(accepted_state.0, "active");
    assert_eq!(accepted_state.1, 2);
    assert_eq!(accepted_state.2, PASSWORD_HASH);
    assert!(!accepted_state.3);
    assert!(accepted_state.4.is_some());
    assert!(accepted_state.5.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, SqliteDecimal>("SELECT balance_amount FROM users WHERE id=?1")
            .bind(created.user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap()
            .into_inner(),
        decimal(EXACT_BALANCE)
    );
    assert!(
        repository
            .accept_invitation(INVITATION_ID, b"invitation-token-hash", PASSWORD_HASH)
            .await
            .unwrap()
            .is_none()
    );

    let mut rounded_input = invite_input(
        "rounded-invite@example.test",
        "Rounded invitation",
        None,
        None,
    );
    rounded_input.initial_balance_amount = decimal("0.000000005");
    let rounded_invitation = repository
        .invite_user(
            ADMIN_ID,
            rounded_input,
            Uuid::new_v4(),
            b"rounded-invitation-token-hash",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, SqliteDecimal>("SELECT balance_amount FROM users WHERE id=?1")
            .bind(rounded_invitation.user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap()
            .into_inner(),
        decimal("0.00000001")
    );
    let rounded_audit = sqlx::query_scalar::<_, String>(
        "SELECT after_redacted FROM audit_logs \
         WHERE action='invite' AND object_id=?1",
    )
    .bind(rounded_invitation.user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(rounded_audit.contains("\"balance_amount\":\"0.00000001\""));

    let concurrent = repository
        .invite_user(
            ADMIN_ID,
            invite_input(
                "concurrent@example.test",
                "Concurrent invitation",
                None,
                None,
            ),
            CONCURRENT_INVITATION_ID,
            b"concurrent-token-hash",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let first_repository = repository.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_repository
            .accept_invitation(
                CONCURRENT_INVITATION_ID,
                b"concurrent-token-hash",
                "$argon2id$concurrent-a",
            )
            .await
            .unwrap()
    });
    let second_repository = repository.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_repository
            .accept_invitation(
                CONCURRENT_INVITATION_ID,
                b"concurrent-token-hash",
                "$argon2id$concurrent-b",
            )
            .await
            .unwrap()
    });
    barrier.wait().await;
    let outcomes = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_some()).count(),
        1
    );
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_none()).count(),
        1
    );

    let state = sqlx::query_as::<_, (String, i64, Option<DateTime<Utc>>)>(
        "SELECT status,auth_version,password_changed_at FROM users WHERE id=?1",
    )
    .bind(concurrent.user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(state.0, "active");
    assert_eq!(state.1, 2);
    assert!(state.2.is_some());
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT accepted_at FROM user_invitations WHERE id=?1"
        )
        .bind(CONCURRENT_INVITATION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_some()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action='activate' AND object_id=?1"
        )
        .bind(concurrent.user_id)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        1
    );

    let invitation_audits = sqlx::query_as::<_, (String, String, String)>(
        "SELECT action,before_redacted,after_redacted FROM audit_logs \
         WHERE object_id IN (?1,?2) AND action IN ('invite','activate')",
    )
    .bind(created.user_id)
    .bind(concurrent.user_id)
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert_eq!(invitation_audits.len(), 4);
    for (action, before_redacted, after_redacted) in invitation_audits {
        assert_eq!(
            serde_json::from_str::<Value>(&before_redacted).unwrap(),
            Value::Object(Default::default())
        );
        for payload in [&before_redacted, &after_redacted] {
            for sensitive in [
                "token_hash",
                "password_hash",
                "invitation_token",
                "invitation-token-hash",
                "concurrent-token-hash",
                PASSWORD_HASH,
                "$argon2id$concurrent-a",
                "$argon2id$concurrent-b",
            ] {
                assert!(
                    !payload.contains(sensitive),
                    "{sensitive} leaked in {payload}"
                );
            }
        }
        let after: Value = serde_json::from_str(&after_redacted).unwrap();
        if action == "invite" {
            assert_object_keys(
                &after,
                &[
                    "balance_amount",
                    "default_api_key_policy_id",
                    "display_name",
                    "email",
                    "id",
                    "role",
                    "status",
                    "updated_at",
                    "user_group_id",
                ],
            );
            assert_eq!(after["balance_amount"].as_str(), Some(EXACT_BALANCE));
        } else {
            assert_object_keys(&after, &["auth_version", "status"]);
        }
    }

    let rollback = repository
        .invite_user(
            ADMIN_ID,
            invite_input(
                "accept-rollback@example.test",
                "Accept rollback",
                None,
                None,
            ),
            ROLLBACK_INVITATION_ID,
            b"accept-rollback-token-hash",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    install_audit_failure(&database.pool).await;
    assert!(matches!(
        repository
            .accept_invitation(
                ROLLBACK_INVITATION_ID,
                b"accept-rollback-token-hash",
                "$argon2id$accept-rollback",
            )
            .await,
        Err(RepositoryError::Sql(_))
    ));
    let rollback_state = sqlx::query_as::<_, (String, i64, Option<String>, Option<DateTime<Utc>>)>(
        "SELECT u.status,u.auth_version,u.password_hash,i.accepted_at \
         FROM users AS u JOIN user_invitations AS i ON i.user_id=u.id WHERE i.id=?1",
    )
    .bind(ROLLBACK_INVITATION_ID)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(rollback_state.0, "invited");
    assert_eq!(rollback_state.1, 1);
    assert!(rollback_state.2.is_none());
    assert!(rollback_state.3.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action='activate' AND object_id=?1"
        )
        .bind(rollback.user_id)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn sqlite_reinvitation_enforces_eligibility_revokes_old_access_and_advances_auth_version() {
    let database = migrated_pool().await;
    insert_admin(&database.pool).await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    let original = repository
        .invite_user(
            ADMIN_ID,
            invite_input("reinvite@example.test", "Reinvited user", None, None),
            INVITATION_ID,
            b"old-invitation-token-hash",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE users SET status='suspended' WHERE id=?1")
        .bind(original.user_id)
        .execute(&database.pool)
        .await
        .unwrap();
    insert_session(&database.pool, SESSION_ID, original.user_id).await;

    let replacement = repository
        .reissue_invitation(
            ADMIN_ID,
            original.user_id,
            REPLACEMENT_INVITATION_ID,
            b"replacement-token-hash",
            Duration::from_secs(7200),
        )
        .await
        .unwrap();
    assert_eq!(replacement.user_id, original.user_id);
    let user_state =
        sqlx::query_as::<_, (String, i64)>("SELECT status,auth_version FROM users WHERE id=?1")
            .bind(original.user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(user_state, ("invited".to_owned(), 2));
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_invitations WHERE id=?1"
        )
        .bind(INVITATION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_some()
    );
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1"
        )
        .bind(SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_some()
    );
    assert!(
        repository
            .accept_invitation(INVITATION_ID, b"old-invitation-token-hash", PASSWORD_HASH)
            .await
            .unwrap()
            .is_none()
    );
    let accepted = repository
        .accept_invitation(
            REPLACEMENT_INVITATION_ID,
            b"replacement-token-hash",
            PASSWORD_HASH,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.auth_version, 3);
    assert!(matches!(
        repository
            .reissue_invitation(
                ADMIN_ID,
                original.user_id,
                Uuid::new_v4(),
                b"ineligible-active-token-hash",
                Duration::from_secs(3600),
            )
            .await,
        Err(RepositoryError::Validation)
    ));

    insert_user(
        &database.pool,
        ELIGIBILITY_USER_ID,
        Some("active-passwordless@example.test"),
        "Active passwordless",
        "user",
        "active",
        None,
        "0",
    )
    .await;
    insert_user(
        &database.pool,
        EMAILLESS_USER_ID,
        None,
        "Emailless invited",
        "user",
        "invited",
        None,
        "0",
    )
    .await;
    insert_user(
        &database.pool,
        SYSTEM_USER_ID,
        Some("system-invited@example.test"),
        "System invited",
        "user",
        "invited",
        None,
        "0",
    )
    .await;
    sqlx::query("UPDATE users SET is_system=1 WHERE id=?1")
        .bind(SYSTEM_USER_ID)
        .execute(&database.pool)
        .await
        .unwrap();
    for user_id in [ELIGIBILITY_USER_ID, EMAILLESS_USER_ID] {
        assert!(matches!(
            repository
                .reissue_invitation(
                    ADMIN_ID,
                    user_id,
                    Uuid::new_v4(),
                    b"ineligible-token-hash",
                    Duration::from_secs(3600),
                )
                .await,
            Err(RepositoryError::Validation)
        ));
    }
    for user_id in [SYSTEM_USER_ID, MISSING_ID] {
        assert!(matches!(
            repository
                .reissue_invitation(
                    ADMIN_ID,
                    user_id,
                    Uuid::new_v4(),
                    b"not-found-token-hash",
                    Duration::from_secs(3600),
                )
                .await,
            Err(RepositoryError::NotFound)
        ));
    }

    let audit = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT before_redacted,after_redacted,correlation_id FROM audit_logs \
         WHERE action='reinvite' AND object_id=?1",
    )
    .bind(original.user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    let correlation_id = replacement.correlation_id.to_string();
    assert_eq!(audit.2.as_deref(), Some(correlation_id.as_str()));
    let before: Value = serde_json::from_str(&audit.0).unwrap();
    let after: Value = serde_json::from_str(&audit.1).unwrap();
    assert_object_keys(
        &before,
        &[
            "balance_amount",
            "default_api_key_policy_id",
            "display_name",
            "email",
            "id",
            "role",
            "status",
            "updated_at",
            "user_group_id",
        ],
    );
    assert_object_keys(
        &after,
        &[
            "balance_amount",
            "default_api_key_policy_id",
            "display_name",
            "email",
            "id",
            "invitation_expires_at",
            "invitation_id",
            "role",
            "status",
            "updated_at",
            "user_group_id",
        ],
    );
    assert_eq!(before["status"], "suspended");
    assert_eq!(after["status"], "invited");
    assert_eq!(before["balance_amount"].as_str(), Some(EXACT_BALANCE));
    assert_eq!(after["balance_amount"].as_str(), Some(EXACT_BALANCE));

    let concurrent = repository
        .invite_user(
            ADMIN_ID,
            invite_input(
                "concurrent-reissue@example.test",
                "Concurrent reissue",
                None,
                None,
            ),
            CONCURRENT_INVITATION_ID,
            b"concurrent-reissue-original",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let first_repository = repository.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_repository
            .reissue_invitation(
                ADMIN_ID,
                concurrent.user_id,
                CONCURRENT_REISSUE_A_ID,
                b"concurrent-reissue-a",
                Duration::from_secs(3600),
            )
            .await
    });
    let second_repository = repository.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_repository
            .reissue_invitation(
                ADMIN_ID,
                concurrent.user_id,
                CONCURRENT_REISSUE_B_ID,
                b"concurrent-reissue-b",
                Duration::from_secs(3600),
            )
            .await
    });
    barrier.wait().await;
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT auth_version FROM users WHERE id=?1")
            .bind(concurrent.user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM user_invitations \
             WHERE user_id=?1 AND accepted_at IS NULL AND revoked_at IS NULL"
        )
        .bind(concurrent.user_id)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action='reinvite' AND object_id=?1"
        )
        .bind(concurrent.user_id)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        2
    );
}

#[tokio::test]
async fn sqlite_invitation_mutations_enforce_guards_and_roll_back_on_audit_failure() {
    let database = migrated_pool().await;
    insert_admin(&database.pool).await;
    insert_user(
        &database.pool,
        INACTIVE_ADMIN_ID,
        Some("inactive-admin@example.test"),
        "Inactive SQLite administrator",
        "admin",
        "suspended",
        Some("$argon2id$inactive-admin"),
        "0",
    )
    .await;
    insert_policy(
        &database.pool,
        ENABLED_POLICY_ID,
        "Enabled invitation policy",
        true,
    )
    .await;
    insert_policy(
        &database.pool,
        DISABLED_POLICY_ID,
        "Disabled invitation policy",
        false,
    )
    .await;
    let repository = SqliteAuthRepository::new(database.pool.clone());

    for actor_id in [MISSING_ID, INACTIVE_ADMIN_ID] {
        assert!(matches!(
            repository
                .invite_user(
                    actor_id,
                    invite_input(
                        &format!("guard-{actor_id}@example.test"),
                        &format!("Guard {actor_id}"),
                        None,
                        None,
                    ),
                    Uuid::new_v4(),
                    b"guard-token-hash",
                    Duration::from_secs(3600),
                )
                .await,
            Err(RepositoryError::NotFound)
        ));
    }
    assert!(matches!(
        repository
            .create_registration_invitation_code(
                INACTIVE_ADMIN_ID,
                b"inactive-admin-code-hash",
                code_input(
                    "Inactive administrator code",
                    Some(1),
                    true,
                    DEFAULT_USER_GROUP_ID,
                    "0",
                ),
            )
            .await,
        Err(RepositoryError::NotFound)
    ));
    assert!(matches!(
        repository
            .invite_user(
                ADMIN_ID,
                invite_input(
                    "missing-group@example.test",
                    "Missing group",
                    Some(MISSING_ID),
                    None,
                ),
                Uuid::new_v4(),
                b"missing-group-token-hash",
                Duration::from_secs(3600),
            )
            .await,
        Err(RepositoryError::Validation)
    ));
    for policy_id in [MISSING_ID, DISABLED_POLICY_ID] {
        assert!(matches!(
            repository
                .invite_user(
                    ADMIN_ID,
                    invite_input(
                        &format!("policy-{policy_id}@example.test"),
                        &format!("Policy guard {policy_id}"),
                        None,
                        Some(policy_id),
                    ),
                    Uuid::new_v4(),
                    b"policy-guard-token-hash",
                    Duration::from_secs(3600),
                )
                .await,
            Err(RepositoryError::Validation)
        ));
    }
    let mut admin_invite = invite_input(
        "invited-admin@example.test",
        "Invited administrator",
        None,
        Some(ENABLED_POLICY_ID),
    );
    admin_invite.role = UserRole::Admin;
    let invited_admin = repository
        .invite_user(
            ADMIN_ID,
            admin_invite,
            Uuid::new_v4(),
            b"invited-admin-token-hash",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_as::<_, (String, Uuid)>("SELECT role,user_group_id FROM users WHERE id=?1")
            .bind(invited_admin.user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        ("admin".to_owned(), DEFAULT_ADMIN_GROUP_ID)
    );

    install_audit_failure(&database.pool).await;
    assert!(matches!(
        repository
            .invite_user(
                ADMIN_ID,
                invite_input(
                    "invite-rollback@example.test",
                    "Invite rollback",
                    None,
                    Some(ENABLED_POLICY_ID),
                ),
                ROLLBACK_INVITATION_ID,
                b"invite-rollback-token-hash",
                Duration::from_secs(3600),
            )
            .await,
        Err(RepositoryError::Sql(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users WHERE email=?1")
            .bind("invite-rollback@example.test")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_invitations WHERE id=?1")
            .bind(ROLLBACK_INVITATION_ID)
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );
    remove_audit_failure(&database.pool).await;

    let original = repository
        .invite_user(
            ADMIN_ID,
            invite_input(
                "reinvite-rollback@example.test",
                "Reinvite rollback",
                None,
                Some(ENABLED_POLICY_ID),
            ),
            ROLLBACK_INVITATION_ID,
            b"original-rollback-token-hash",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    insert_session(&database.pool, ROLLBACK_SESSION_ID, original.user_id).await;
    install_audit_failure(&database.pool).await;
    assert!(matches!(
        repository
            .reissue_invitation(
                ADMIN_ID,
                original.user_id,
                ROLLBACK_REPLACEMENT_ID,
                b"replacement-rollback-token-hash",
                Duration::from_secs(3600),
            )
            .await,
        Err(RepositoryError::Sql(_))
    ));
    let user_state =
        sqlx::query_as::<_, (String, i64)>("SELECT status,auth_version FROM users WHERE id=?1")
            .bind(original.user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(user_state, ("invited".to_owned(), 1));
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_invitations WHERE id=?1"
        )
        .bind(ROLLBACK_INVITATION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_none()
    );
    assert!(
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT revoked_at FROM user_sessions WHERE id=?1"
        )
        .bind(ROLLBACK_SESSION_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_invitations WHERE id=?1")
            .bind(ROLLBACK_REPLACEMENT_ID)
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );
}
