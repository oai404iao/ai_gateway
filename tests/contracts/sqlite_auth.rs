//! SQLite Console identity, session, invitation, and registration-code contracts.

use super::*;
use ai_gateway::{
    domain::{ConsoleSessionPurpose, UserRole},
    persistence::{
        DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID, InviteUserInput, RegistrationAttempt,
        RegistrationInvitationCodeInput, RepositoryError, SessionRotation,
        sqlite::{SqliteAmount, SqliteAuthRepository, SqliteUuid},
    },
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use std::{str::FromStr, sync::Arc, time::Duration};
use uuid::Uuid;

const USER_ID: Uuid = Uuid::from_u128(0x301);
const ADMIN_ID: Uuid = Uuid::from_u128(0x302);
const SESSION_ID: Uuid = Uuid::from_u128(0x311);
const OTHER_SESSION_ID: Uuid = Uuid::from_u128(0x312);
const INVITATION_ID: Uuid = Uuid::from_u128(0x321);
const REISSUED_INVITATION_ID: Uuid = Uuid::from_u128(0x322);
const POLICY_ID: Uuid = Uuid::from_u128(0x341);
const PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA";
const OLD_HASH: &[u8] = b"old-refresh-hash";
const NEXT_HASH: &[u8] = b"next-refresh-hash";

struct AuditEntry {
    before: Value,
    after: Value,
    actor_type: String,
    actor_role: Option<String>,
    object_type: String,
}

async fn auth() -> (tempfile::TempDir, Arc<SqliteDatabase>, SqliteAuthRepository) {
    let (directory, database) = database().await;
    assert_eq!(database.install_schema().await.unwrap(), 9);
    let database = Arc::new(database);
    let repository = SqliteAuthRepository::new(Arc::clone(&database));
    (directory, database, repository)
}

async fn execute(database: &SqliteDatabase, sql: &str) {
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::Executor::execute(&mut *transaction, sqlx::AssertSqlSafe(sql.to_owned()))
        .await
        .unwrap();
    transaction.commit().await.unwrap();
}

async fn insert_member(
    database: &SqliteDatabase,
    id: Uuid,
    email: &str,
    name: &str,
    role: &str,
    group: Uuid,
) {
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query(
        "INSERT INTO users \
         (id,email,display_name,role,status,password_hash,password_changed_at,user_group_id) \
         VALUES (?,?,?,?,'active',?,ag_now(),?)",
    )
    .bind(SqliteUuid(id))
    .bind(email)
    .bind(name)
    .bind(role)
    .bind(PASSWORD_HASH)
    .bind(SqliteUuid(group))
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
}

async fn seed_user_and_admin(database: &SqliteDatabase) {
    insert_member(
        database,
        USER_ID,
        "User@Example.Test",
        "Auth user",
        "user",
        DEFAULT_USER_GROUP_ID,
    )
    .await;
    insert_member(
        database,
        ADMIN_ID,
        "admin@example.test",
        "Auth admin",
        "admin",
        DEFAULT_ADMIN_GROUP_ID,
    )
    .await;
}

async fn scalar<T>(database: &SqliteDatabase, sql: &str) -> T
where
    for<'r> T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + Unpin,
{
    let mut reader = database.acquire_read().await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut *reader)
        .await
        .unwrap()
}

async fn audit(database: &SqliteDatabase, action: &str, object_id: Uuid) -> AuditEntry {
    let mut reader = database.acquire_read().await.unwrap();
    let (before, after, actor_type, actor_role, object_type) =
        sqlx::query_as::<_, (String, String, String, Option<String>, String)>(
            "SELECT before_redacted,after_redacted,actor_type,actor_role,object_type \
             FROM audit_logs WHERE action=? AND object_id=?",
        )
        .bind(action)
        .bind(SqliteUuid(object_id))
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    AuditEntry {
        before: serde_json::from_str(&before).unwrap(),
        after: serde_json::from_str(&after).unwrap(),
        actor_type,
        actor_role,
        object_type,
    }
}

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn decimal(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

fn registration_input(name: &str, balance: &str) -> RegistrationInvitationCodeInput {
    RegistrationInvitationCodeInput {
        name: name.into(),
        max_uses: Some(1),
        expires_at: Some(timestamp("2030-01-01T00:00:00Z")),
        enabled: true,
        user_group_id: DEFAULT_USER_GROUP_ID,
        initial_balance_amount: decimal(balance),
    }
}

#[tokio::test]
async fn login_identity_sessions_and_profile_follow_case_insensitive_identity() {
    let (_directory, database, repository) = auth().await;
    insert_member(
        &database,
        USER_ID,
        "User@Example.Test",
        "SQLite auth user",
        "user",
        DEFAULT_USER_GROUP_ID,
    )
    .await;

    // Case-insensitive lookup matches PostgreSQL lower(); surrounding whitespace is removed by
    // the application layer before the repository is called.
    let login = repository
        .find_login_user("USER@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(login.id, USER_ID);
    assert_eq!(login.email.as_deref(), Some("User@Example.Test"));
    assert_eq!(login.role, "user");
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

    let password_user = repository.password_user(USER_ID).await.unwrap().unwrap();
    assert_eq!(password_user.password_hash.as_deref(), Some(PASSWORD_HASH));
    assert_eq!(password_user.auth_version, 1);

    let far_future = timestamp("9999-01-02T03:04:05.123456Z");
    repository
        .create_session(
            SESSION_ID,
            USER_ID,
            OLD_HASH,
            far_future,
            Some("SQLite contract agent"),
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();

    let identity = repository
        .validate_console_identity(USER_ID, SESSION_ID, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(identity.session_id, SESSION_ID);
    assert_eq!(identity.session_purpose, "normal");
    assert_eq!(identity.expires_at, far_future);
    assert!(
        repository
            .validate_console_identity(USER_ID, SESSION_ID, 2)
            .await
            .unwrap()
            .is_none()
    );

    let rotation = repository
        .rotate_session(SESSION_ID, OLD_HASH, NEXT_HASH, far_future, Some("rotated"))
        .await
        .unwrap();
    let SessionRotation::Rotated {
        user,
        refresh_expires_at,
    } = rotation
    else {
        panic!("the live session must rotate");
    };
    assert_eq!(user.id, USER_ID);
    assert_eq!(refresh_expires_at, far_future);
    assert!(matches!(
        repository
            .rotate_session(SESSION_ID, OLD_HASH, b"unused", far_future, None)
            .await
            .unwrap(),
        SessionRotation::Replayed
    ));
    assert!(
        scalar::<Option<String>>(
            &database,
            "SELECT revoked_at FROM user_sessions WHERE id='00000000-0000-0000-0000-000000000311'"
        )
        .await
        .is_some()
    );

    let profile = repository.profile(USER_ID).await.unwrap().unwrap();
    assert_eq!(profile.display_name, "SQLite auth user");
    assert_eq!(profile.status, "active");
    assert_eq!(profile.balance_amount, Decimal::ZERO);
}

#[tokio::test]
async fn password_change_and_session_administration_update_only_live_state() {
    let (_directory, database, repository) = auth().await;
    seed_user_and_admin(&database).await;

    let far_future = timestamp("9999-01-02T03:04:05.123456Z");
    repository
        .create_session(
            SESSION_ID,
            USER_ID,
            OLD_HASH,
            far_future,
            Some("agent"),
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();
    repository
        .create_session(
            OTHER_SESSION_ID,
            USER_ID,
            OLD_HASH,
            far_future,
            None,
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();

    let sessions = repository
        .sessions_for_user(USER_ID, SESSION_ID)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions[0].is_current);
    assert_eq!(
        serde_json::to_value(&sessions[0].state).unwrap(),
        serde_json::json!("active")
    );
    assert_eq!(sessions[0].user_agent.as_deref(), Some("agent"));

    assert_eq!(
        repository
            .revoke_other_sessions(USER_ID, SESSION_ID)
            .await
            .unwrap(),
        1,
        "only the sibling live session is revoked"
    );
    assert_eq!(
        repository
            .revoke_other_sessions(USER_ID, SESSION_ID)
            .await
            .unwrap(),
        0
    );
    assert!(
        !repository
            .revoke_session_for_user(USER_ID, OTHER_SESSION_ID)
            .await
            .unwrap(),
        "the sibling was already revoked"
    );
    assert!(
        !repository
            .revoke_session_for_user(USER_ID, OTHER_SESSION_ID)
            .await
            .unwrap(),
        "the sibling stays revoked"
    );

    assert!(
        repository
            .change_password(USER_ID, "$argon2id$replacement")
            .await
            .unwrap()
    );
    let after = repository.password_user(USER_ID).await.unwrap().unwrap();
    assert_eq!(after.auth_version, 2);
    assert_eq!(
        after.password_hash.as_deref(),
        Some("$argon2id$replacement")
    );
    assert!(
        repository
            .validate_console_identity(USER_ID, SESSION_ID, 2)
            .await
            .unwrap()
            .is_none(),
        "the password change revoked the live session"
    );

    let updated = repository
        .update_display_name(USER_ID, "Renamed")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.display_name, "Renamed");
    assert!(
        repository
            .update_display_name(Uuid::from_u128(0x999), "Nobody")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        scalar::<String>(
            &database,
            "SELECT CASE WHEN updated_at > created_at THEN 'newer' ELSE 'stale' END \
             FROM users WHERE id='00000000-0000-0000-0000-000000000301'"
        )
        .await
            == "newer"
    );

    repository.revoke_all_sessions(USER_ID).await.unwrap();
    assert!(
        repository
            .sessions_for_user(USER_ID, SESSION_ID)
            .await
            .unwrap()
            .iter()
            .all(|session| serde_json::to_value(&session.state).unwrap() == "revoked")
    );
    assert!(
        !repository
            .change_password(Uuid::from_u128(0x999), "unused")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn temporary_password_issue_and_completion_preserve_audit_semantics() {
    let (_directory, database, repository) = auth().await;
    seed_user_and_admin(&database).await;

    assert!(matches!(
        repository
            .issue_temporary_password(
                USER_ID,
                1,
                USER_ID,
                "$argon2id$temp",
                Duration::from_secs(3600)
            )
            .await,
        Err(RepositoryError::CannotResetSelf)
    ));

    let created = repository
        .issue_temporary_password(
            ADMIN_ID,
            1,
            USER_ID,
            "$argon2id$temp",
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    assert_eq!(created.user_id, USER_ID);
    assert!(created.expires_at > Utc::now());

    let issue = audit(&database, "issue_temporary_password", USER_ID).await;
    assert_eq!(issue.actor_type, "user");
    assert_eq!(issue.actor_role.as_deref(), Some("admin"));
    assert_eq!(issue.object_type, "user");
    assert_eq!(issue.after["password_change_required"], true);
    assert_eq!(issue.after["balance_amount"], 0);
    assert_eq!(issue.after["can_reissue_invitation"], false);
    assert!(!issue.before.to_string().contains("password_hash"));

    let bumped = repository.password_user(USER_ID).await.unwrap().unwrap();
    assert_eq!(bumped.auth_version, 2);
    assert!(bumped.password_change_required);

    let far_future = timestamp("9999-01-02T03:04:05.123456Z");
    repository
        .create_session(
            SESSION_ID,
            USER_ID,
            OLD_HASH,
            far_future,
            None,
            ConsoleSessionPurpose::PasswordChange,
        )
        .await
        .unwrap();
    assert!(
        repository
            .complete_temporary_password(USER_ID, SESSION_ID, 1, "$argon2id$x")
            .await
            .unwrap()
            .is_none()
    );
    let completed = repository
        .complete_temporary_password(USER_ID, SESSION_ID, 2, "$argon2id$final")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.auth_version, 3);
    assert_eq!(completed.session_purpose, ConsoleSessionPurpose::Normal);
    assert!(completed.temporary_password_expires_at.is_none());
    let completion = audit(&database, "complete_password_reset", USER_ID).await;
    assert_eq!(completion.after["password_change_required"], false);
    assert_eq!(
        completion.after["temporary_password_expires_at"],
        Value::Null
    );

    assert!(
        matches!(
            repository
                .issue_temporary_password(
                    ADMIN_ID,
                    1,
                    Uuid::from_u128(0x998),
                    "unused",
                    Duration::from_secs(60)
                )
                .await,
            Err(RepositoryError::NotFound)
        ),
        "an unknown target is not resettable"
    );
    assert!(
        matches!(
            repository
                .issue_temporary_password(ADMIN_ID, 99, USER_ID, "unused", Duration::from_secs(60))
                .await,
            Err(RepositoryError::NotFound)
        ),
        "a stale actor version is not an authorized administrator"
    );
    assert!(
        matches!(
            repository
                .issue_temporary_password(USER_ID, 3, ADMIN_ID, "unused", Duration::from_secs(60))
                .await,
            Err(RepositoryError::NotFound)
        ),
        "a non-administrator actor cannot reset a password"
    );
    assert!(matches!(
        repository
            .issue_temporary_password(ADMIN_ID, 1, ADMIN_ID, "unused", Duration::from_secs(60))
            .await,
        Err(RepositoryError::CannotResetSelf)
    ));
}

#[tokio::test]
async fn operator_reset_and_bootstrap_admin_enforce_single_active_admin() {
    let (_directory, database, repository) = auth().await;
    assert!(matches!(
        repository
            .bootstrap_admin(" ", "Blank", PASSWORD_HASH)
            .await,
        Err(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .reset_active_admin_password("nobody@example.test", PASSWORD_HASH)
            .await,
        Ok(false)
    ));

    // The stored address keeps the caller's casing; identity lookups go through ag_lower.
    let admin_id = repository
        .bootstrap_admin("Root@Example.Test", "Root Admin", PASSWORD_HASH)
        .await
        .unwrap();
    assert!(matches!(
        repository
            .bootstrap_admin("second@example.test", "Second Admin", PASSWORD_HASH)
            .await,
        Err(RepositoryError::Conflict)
    ));
    let bootstrap = audit(&database, "bootstrap", admin_id).await;
    assert_eq!(bootstrap.actor_type, "system");
    assert_eq!(bootstrap.before, serde_json::json!({}));
    assert_eq!(bootstrap.after["email"], "Root@Example.Test");
    assert_eq!(bootstrap.after["role"], "admin");

    let far_future = timestamp("9999-01-02T03:04:05.123456Z");
    repository
        .create_session(
            SESSION_ID,
            admin_id,
            OLD_HASH,
            far_future,
            None,
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();
    // Lookup uses ag_lower, matching PostgreSQL lower() for the ASCII case.
    assert!(
        repository
            .reset_active_admin_password("ROOT@example.test", "$argon2id$reset")
            .await
            .unwrap()
    );
    let reset = repository.password_user(admin_id).await.unwrap().unwrap();
    assert_eq!(reset.auth_version, 2);
    assert_eq!(reset.password_hash.as_deref(), Some("$argon2id$reset"));
    let reset_audit = audit(&database, "reset_password", admin_id).await;
    assert_eq!(reset_audit.actor_type, "system");
    assert_eq!(reset_audit.actor_role, None);
    assert_eq!(reset_audit.after["auth_version"], 2);
    assert_eq!(
        scalar::<i64>(
            &database,
            "SELECT count(*) FROM user_sessions WHERE revoked_at IS NOT NULL"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn invitations_reissue_and_acceptance_follow_the_lifecycle() {
    let (_directory, database, repository) = auth().await;
    seed_user_and_admin(&database).await;
    execute(
        &database,
        "INSERT INTO api_key_policies (id,name) \
         VALUES ('00000000-0000-0000-0000-000000000341','SQLite policy')",
    )
    .await;

    assert!(
        matches!(
            repository
                .invite_user(
                    Uuid::from_u128(0x997),
                    InviteUserInput {
                        email: "invitee@example.test".into(),
                        display_name: "Invitee".into(),
                        role: UserRole::User,
                        initial_balance_amount: Decimal::ZERO,
                        user_group_id: None,
                        default_api_key_policy_id: None,
                    },
                    INVITATION_ID,
                    OLD_HASH,
                    Duration::from_secs(3600),
                )
                .await,
            Err(RepositoryError::NotFound)
        ),
        "only an active administrator may invite"
    );
    assert!(matches!(
        repository
            .invite_user(
                ADMIN_ID,
                InviteUserInput {
                    email: "invitee@example.test".into(),
                    display_name: "Invitee".into(),
                    role: UserRole::User,
                    initial_balance_amount: decimal("-1"),
                    user_group_id: None,
                    default_api_key_policy_id: None,
                },
                INVITATION_ID,
                OLD_HASH,
                Duration::from_secs(3600),
            )
            .await,
        Err(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .invite_user(
                ADMIN_ID,
                InviteUserInput {
                    email: "invitee@example.test".into(),
                    display_name: "Invitee".into(),
                    role: UserRole::User,
                    initial_balance_amount: Decimal::ZERO,
                    user_group_id: None,
                    default_api_key_policy_id: Some(Uuid::from_u128(0x994)),
                },
                INVITATION_ID,
                OLD_HASH,
                Duration::from_secs(3600),
            )
            .await,
        Err(RepositoryError::Validation)
    ));

    let created = repository
        .invite_user(
            ADMIN_ID,
            InviteUserInput {
                email: "Invitee@Example.Test".into(),
                display_name: "Invitee".into(),
                role: UserRole::User,
                initial_balance_amount: decimal("25.50"),
                user_group_id: None,
                default_api_key_policy_id: Some(POLICY_ID),
            },
            INVITATION_ID,
            OLD_HASH,
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    assert_eq!(created.invitation_id, INVITATION_ID);

    let invitation = audit(&database, "invite", created.user_id).await;
    assert_eq!(invitation.actor_role.as_deref(), Some("admin"));
    assert_eq!(invitation.before, serde_json::json!({}));
    assert_eq!(invitation.after["status"], "invited");
    assert_eq!(invitation.after["email"], "Invitee@Example.Test");
    assert_eq!(
        invitation.after["default_api_key_policy_id"],
        POLICY_ID.to_string()
    );
    assert_eq!(invitation.after["balance_amount"], "25.50");
    let (group, balance) = {
        let mut reader = database.acquire_read().await.unwrap();
        sqlx::query_as::<_, (String, SqliteAmount)>(
            "SELECT user_group_id,balance_amount FROM users WHERE id=?",
        )
        .bind(SqliteUuid(created.user_id))
        .fetch_one(&mut *reader)
        .await
        .unwrap()
    };
    assert_eq!(group, DEFAULT_USER_GROUP_ID.to_string());
    assert_eq!(balance.0, decimal("25.50000000"));
    assert_eq!(
        scalar::<String>(
            &database,
            &format!(
                "SELECT balance_amount FROM users WHERE id='{}'",
                created.user_id
            )
        )
        .await,
        "25.5",
        "SQLite stores the canonical trailing-zero-free representation"
    );

    let reissued = repository
        .reissue_invitation(
            ADMIN_ID,
            created.user_id,
            REISSUED_INVITATION_ID,
            NEXT_HASH,
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    assert_eq!(reissued.invitation_id, REISSUED_INVITATION_ID);
    let reissue = audit(&database, "reinvite", created.user_id).await;
    assert_eq!(reissue.before["status"], "invited");
    assert_eq!(reissue.after["status"], "invited");
    assert_eq!(
        reissue.after["invitation_id"],
        REISSUED_INVITATION_ID.to_string()
    );
    assert_eq!(
        {
            let mut reader = database.acquire_read().await.unwrap();
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM user_invitations \
                 WHERE user_id=? AND revoked_at IS NOT NULL",
            )
            .bind(SqliteUuid(created.user_id))
            .fetch_one(&mut *reader)
            .await
            .unwrap()
        },
        1,
        "reissue revokes the superseded invitation"
    );

    assert!(
        repository
            .accept_invitation(INVITATION_ID, OLD_HASH, "$argon2id$chosen")
            .await
            .unwrap()
            .is_none(),
        "the superseded invitation must not be accepted"
    );
    assert!(
        repository
            .accept_invitation(REISSUED_INVITATION_ID, OLD_HASH, "$argon2id$chosen")
            .await
            .unwrap()
            .is_none(),
        "the presented token must match the stored hash"
    );
    let session_user = repository
        .accept_invitation(REISSUED_INVITATION_ID, NEXT_HASH, "$argon2id$chosen")
        .await
        .unwrap()
        .unwrap();
    let activation = audit(&database, "activate", created.user_id).await;
    assert_eq!(activation.before, serde_json::json!({}));
    assert_eq!(activation.after["status"], "active");
    assert_eq!(activation.after["auth_version"], session_user.auth_version);
    let accepted = repository
        .find_login_user("invitee@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.status, "active");
    assert_eq!(accepted.password_hash.as_deref(), Some("$argon2id$chosen"));
    assert_eq!(accepted.auth_version, session_user.auth_version);
    assert!(
        repository
            .accept_invitation(REISSUED_INVITATION_ID, NEXT_HASH, "$argon2id$again")
            .await
            .unwrap()
            .is_none(),
        "an accepted invitation is one-shot"
    );
}

#[tokio::test]
async fn registration_codes_validate_conflicts_and_normalize_writes() {
    let (_directory, database, repository) = auth().await;
    seed_user_and_admin(&database).await;

    let input = registration_input("One seat", "25.50");
    assert!(matches!(
        repository
            .create_registration_invitation_code(
                ADMIN_ID,
                OLD_HASH,
                RegistrationInvitationCodeInput {
                    max_uses: Some(0),
                    ..input.clone()
                },
            )
            .await,
        Err(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .create_registration_invitation_code(
                ADMIN_ID,
                OLD_HASH,
                RegistrationInvitationCodeInput {
                    name: " ".into(),
                    ..input.clone()
                },
            )
            .await,
        Err(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .create_registration_invitation_code(
                ADMIN_ID,
                OLD_HASH,
                RegistrationInvitationCodeInput {
                    user_group_id: Uuid::from_u128(0x996),
                    ..input.clone()
                },
            )
            .await,
        Err(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .create_registration_invitation_code(Uuid::from_u128(0x993), OLD_HASH, input.clone())
            .await,
        Err(RepositoryError::NotFound)
    ));

    let created = repository
        .create_registration_invitation_code(ADMIN_ID, OLD_HASH, input.clone())
        .await
        .unwrap();
    assert!(matches!(
        repository
            .create_registration_invitation_code(ADMIN_ID, OLD_HASH, input.clone())
            .await,
        Err(RepositoryError::RegistrationInvitationCodeConflict)
    ));

    let create_audit = audit(&database, "create", created.id).await;
    assert_eq!(create_audit.object_type, "registration_invitation_code");
    assert_eq!(create_audit.actor_role.as_deref(), Some("admin"));
    assert_eq!(create_audit.before, serde_json::json!({}));
    assert_eq!(create_audit.after["used_count"], 0);
    assert_eq!(create_audit.after["initial_balance_amount"], "25.50000000");

    let before = repository
        .registration_invitation_code(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.used_count, 0);
    assert_eq!(before.initial_balance_amount, decimal("25.50000000"));
    assert_eq!(
        repository
            .registration_invitation_codes()
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        repository
            .registration_invitation_code(Uuid::from_u128(0x995))
            .await
            .unwrap()
            .is_none()
    );

    let adjusted = RegistrationInvitationCodeInput {
        name: "Adjusted".into(),
        max_uses: Some(3),
        expires_at: None,
        enabled: false,
        user_group_id: DEFAULT_USER_GROUP_ID,
        initial_balance_amount: decimal("75.25"),
    };
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                created.id,
                adjusted.clone(),
                before.updated_at + chrono::Duration::seconds(1),
            )
            .await,
        Err(RepositoryError::Conflict)
    ));
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                created.id,
                RegistrationInvitationCodeInput {
                    max_uses: Some(0),
                    ..adjusted.clone()
                },
                before.updated_at,
            )
            .await,
        Err(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                Uuid::from_u128(0x993),
                created.id,
                adjusted.clone(),
                before.updated_at,
            )
            .await,
        Err(RepositoryError::NotFound)
    ));

    let updated = repository
        .update_registration_invitation_code(
            ADMIN_ID,
            created.id,
            adjusted.clone(),
            before.updated_at,
        )
        .await
        .unwrap();
    assert_eq!(updated.id, created.id);
    let after = repository
        .registration_invitation_code(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        after.updated_at > before.updated_at,
        "the timestamp guard advances the ETag"
    );
    assert_eq!(after.name, "Adjusted");
    assert!(!after.enabled);
    assert_eq!(after.initial_balance_amount, decimal("75.25000000"));
    let update_audit = audit(&database, "update", created.id).await;
    assert_eq!(update_audit.before["name"], "One seat");
    assert_eq!(update_audit.after["name"], "Adjusted");
    assert_eq!(update_audit.after["enabled"], false);

    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                created.id,
                adjusted.clone(),
                after.updated_at + chrono::Duration::seconds(1),
            )
            .await,
        Err(RepositoryError::Conflict)
    ));

    // A second code must not be renameable onto the first one's name.
    let other = repository
        .create_registration_invitation_code(
            ADMIN_ID,
            NEXT_HASH,
            registration_input("Second seat", "1"),
        )
        .await
        .unwrap();
    let other_before = repository
        .registration_invitation_code(other.id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        repository
            .update_registration_invitation_code(
                ADMIN_ID,
                other.id,
                RegistrationInvitationCodeInput {
                    name: after.name.clone(),
                    ..adjusted
                },
                other_before.updated_at,
            )
            .await,
        Err(RepositoryError::RegistrationInvitationCodeConflict)
    ));
    let unchanged = repository
        .registration_invitation_code(other.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.name, "Second seat");
    assert_eq!(unchanged.updated_at, other_before.updated_at);
}

#[tokio::test]
async fn registration_consumes_the_code_once_and_audits_the_projection() {
    let (_directory, database, repository) = auth().await;
    seed_user_and_admin(&database).await;

    let code = repository
        .create_registration_invitation_code(
            ADMIN_ID,
            OLD_HASH,
            registration_input("Reusable", "10"),
        )
        .await
        .unwrap();

    assert!(matches!(
        repository
            .register_with_invitation_code(NEXT_HASH, "a@example.test", "A", PASSWORD_HASH)
            .await
            .unwrap(),
        RegistrationAttempt::InvalidCode
    ));

    let registration = repository
        .register_with_invitation_code(OLD_HASH, " New@Example.Test ", "New User", PASSWORD_HASH)
        .await
        .unwrap();
    let RegistrationAttempt::Registered(user) = registration else {
        panic!("the first registration must succeed");
    };
    assert_eq!(user.email.as_deref(), Some(" New@Example.Test "));
    assert_eq!(user.role, UserRole::User);
    assert_eq!(user.auth_version, 1);
    assert!(user.temporary_password_expires_at.is_none());

    let projected = audit(&database, "register", user.id).await;
    assert_eq!(
        projected.after["registration_invitation_code_id"],
        code.id.to_string()
    );
    assert_eq!(projected.after["balance_amount"], "10.00000000");
    assert_eq!(
        projected.after["user_group_id"],
        DEFAULT_USER_GROUP_ID.to_string()
    );

    let consumed = repository
        .registration_invitation_code(code.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(consumed.used_count, 1);
    assert!(consumed.last_used_at.is_some());
    assert!(
        consumed.updated_at > consumed.created_at,
        "consuming the code advances its timestamp"
    );

    assert!(
        matches!(
            repository
                .register_with_invitation_code(
                    OLD_HASH,
                    "other@example.test",
                    "Other",
                    PASSWORD_HASH
                )
                .await
                .unwrap(),
            RegistrationAttempt::InvalidCode
        ),
        "the use limit is exhausted"
    );
    assert!(
        matches!(
            repository
                .register_with_invitation_code(
                    OLD_HASH,
                    " New@Example.Test ",
                    "Duplicate",
                    PASSWORD_HASH
                )
                .await
                .unwrap(),
            RegistrationAttempt::InvalidCode
        ),
        "the code is exhausted before the duplicate is observed"
    );

    let unlimited = repository
        .create_registration_invitation_code(
            ADMIN_ID,
            NEXT_HASH,
            RegistrationInvitationCodeInput {
                name: "Unlimited".into(),
                max_uses: None,
                expires_at: None,
                enabled: true,
                user_group_id: DEFAULT_USER_GROUP_ID,
                initial_balance_amount: Decimal::ZERO,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(
            repository
                .register_with_invitation_code(
                    NEXT_HASH,
                    " New@Example.Test ",
                    "Duplicate",
                    PASSWORD_HASH
                )
                .await
                .unwrap(),
            RegistrationAttempt::EmailConflict
        ),
        "a duplicate identity is reported once the code itself is valid"
    );
    let used = repository
        .registration_invitation_code(unlimited.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        used.used_count, 0,
        "a rejected registration must not consume a use"
    );
}

/// A session that expires while the listing waits for a read connection must still be reported as
/// `expired`: the state is classified against the time the rows were actually read, not the call
/// start. The reader pool is exhausted first, so the assertion can only hold if the clock is
/// sampled after the fetch.
#[tokio::test]
async fn session_expiry_is_classified_after_the_reader_is_acquired() {
    let (_directory, database, repository) = auth().await;
    insert_member(
        &database,
        USER_ID,
        "User@Example.Test",
        "Auth user",
        "user",
        DEFAULT_USER_GROUP_ID,
    )
    .await;

    let mut readers = Vec::new();
    for _ in 0..4 {
        readers.push(database.acquire_read().await.unwrap());
    }

    let expires_at = Utc::now() + Duration::from_millis(500);
    repository
        .create_session(
            SESSION_ID,
            USER_ID,
            OLD_HASH,
            expires_at,
            None,
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();
    assert!(
        Utc::now() < expires_at,
        "the session must still be live when the listing starts"
    );

    let listing = tokio::spawn({
        let repository = repository.clone();
        async move { repository.sessions_for_user(USER_ID, SESSION_ID).await }
    });

    // Bounded wait instead of a fixed sleep: expire the session while the listing is blocked, then
    // let it read and classify.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while Utc::now() < expires_at {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session never reached its expiry"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !listing.is_finished(),
        "the listing must still be waiting for a read connection"
    );

    drop(readers);
    let sessions = tokio::time::timeout(Duration::from_secs(5), listing)
        .await
        .expect("the listing must finish once a read connection is free")
        .unwrap()
        .unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, SESSION_ID);
    assert!(sessions[0].is_current);
    assert!(sessions[0].expires_at <= Utc::now());
    assert_eq!(
        serde_json::to_value(&sessions[0].state).unwrap(),
        serde_json::json!("expired")
    );
}
