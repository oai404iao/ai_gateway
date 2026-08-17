//! SQLite registration-code and user-invitation persistence.

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use sqlx::{FromRow, Sqlite, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::domain::{ConsoleSessionPurpose, UserRole};

use super::super::super::{
    auth::{
        InvitationCreated, InviteUserInput, RegistrationAttempt, RegistrationInvitationCode,
        RegistrationInvitationCodeInput, RegistrationInvitationCodeMutation, SessionUser,
        normalize_numeric_24_8,
    },
    error::RepositoryError,
    records::{DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID},
};
use super::super::SqliteDecimal;
use super::{SqliteAuthRepository, canonical_email, parse_role, timestamp_text};

impl SqliteAuthRepository {
    pub async fn registration_invitation_codes(
        &self,
    ) -> Result<Vec<RegistrationInvitationCode>, RepositoryError> {
        sqlx::query_as::<_, RegistrationInvitationCode>(
            "SELECT id,name,max_uses,used_count,expires_at,enabled,user_group_id, \
                    initial_balance_amount,created_by,last_used_at,created_at,updated_at \
             FROM registration_invitation_codes ORDER BY created_at DESC,id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn registration_invitation_code(
        &self,
        id: Uuid,
    ) -> Result<Option<RegistrationInvitationCode>, RepositoryError> {
        select_registration_invitation_code(&self.pool, id).await
    }

    pub async fn create_registration_invitation_code(
        &self,
        actor_user_id: Uuid,
        code_hash: &[u8],
        input: RegistrationInvitationCodeInput,
    ) -> Result<RegistrationInvitationCodeMutation, RepositoryError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        validate_registration_invitation_code_input(&input, 0)?;
        ensure_registration_user_group(&mut transaction, input.user_group_id).await?;
        let initial_balance_amount = normalize_numeric_24_8(input.initial_balance_amount);

        let id = Uuid::new_v4();
        let inserted = sqlx::query(
            "INSERT INTO registration_invitation_codes \
             (id,name,code_hash,max_uses,expires_at,enabled,user_group_id, \
              initial_balance_amount,created_by) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(code_hash)
        .bind(input.max_uses)
        .bind(input.expires_at.map(timestamp_text))
        .bind(input.enabled)
        .bind(input.user_group_id)
        .bind(SqliteDecimal::from(initial_balance_amount))
        .bind(actor_user_id)
        .execute(&mut *transaction)
        .await;
        match inserted {
            Ok(_) => {}
            Err(error) if registration_code_unique_violation(&error) => {
                transaction.rollback().await?;
                return Err(RepositoryError::RegistrationInvitationCodeConflict);
            }
            Err(error) => return Err(error.into()),
        }

        let after = registration_invitation_code_in_transaction(&mut transaction, id).await?;
        let correlation_id = Uuid::new_v4();
        insert_registration_invitation_code_audit(
            &mut transaction,
            actor_user_id,
            "create",
            id,
            "{}".to_owned(),
            registration_invitation_code_audit(&after)?,
            correlation_id,
        )
        .await?;
        transaction.commit().await?;
        Ok(RegistrationInvitationCodeMutation { id, correlation_id })
    }

    pub async fn update_registration_invitation_code(
        &self,
        actor_user_id: Uuid,
        id: Uuid,
        input: RegistrationInvitationCodeInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<RegistrationInvitationCodeMutation, RepositoryError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        let before = registration_invitation_code_in_transaction(&mut transaction, id).await?;
        if before.updated_at != expected_updated_at {
            transaction.rollback().await?;
            return Err(RepositoryError::Conflict);
        }
        validate_registration_invitation_code_input(&input, before.used_count)?;
        ensure_registration_user_group(&mut transaction, input.user_group_id).await?;
        let initial_balance_amount = normalize_numeric_24_8(input.initial_balance_amount);

        let updated = sqlx::query(
            "UPDATE registration_invitation_codes SET \
             name=?2,max_uses=?3,expires_at=?4,enabled=?5,user_group_id=?6, \
             initial_balance_amount=?7 \
             WHERE id=?1 AND updated_at=?8",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(input.max_uses)
        .bind(input.expires_at.map(timestamp_text))
        .bind(input.enabled)
        .bind(input.user_group_id)
        .bind(SqliteDecimal::from(initial_balance_amount))
        .bind(etag_timestamp_text(expected_updated_at))
        .execute(&mut *transaction)
        .await;
        let updated = match updated {
            Ok(updated) => updated,
            Err(error) if registration_code_unique_violation(&error) => {
                transaction.rollback().await?;
                return Err(RepositoryError::RegistrationInvitationCodeConflict);
            }
            Err(error) => return Err(error.into()),
        };
        if updated.rows_affected() == 0 {
            transaction.rollback().await?;
            return Err(RepositoryError::Conflict);
        }

        // SQLite's timestamp trigger is AFTER UPDATE, so RETURNING would expose
        // the pre-trigger ETag. Re-read while retaining the write lock.
        let after = registration_invitation_code_in_transaction(&mut transaction, id).await?;
        if after.updated_at <= before.updated_at {
            transaction.rollback().await?;
            return Err(RepositoryError::Conflict);
        }
        let correlation_id = Uuid::new_v4();
        insert_registration_invitation_code_audit(
            &mut transaction,
            actor_user_id,
            "update",
            id,
            registration_invitation_code_audit(&before)?,
            registration_invitation_code_audit(&after)?,
            correlation_id,
        )
        .await?;
        transaction.commit().await?;
        Ok(RegistrationInvitationCodeMutation { id, correlation_id })
    }

    pub async fn register_with_invitation_code(
        &self,
        code_hash: &[u8],
        email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<RegistrationAttempt, RepositoryError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = Utc::now();
        let invitation = sqlx::query_as::<_, RegistrationInvitationCodeForUse>(
            "SELECT id,max_uses,used_count,expires_at,enabled,user_group_id, \
                    initial_balance_amount \
             FROM registration_invitation_codes WHERE code_hash=?1",
        )
        .bind(code_hash)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(invitation) = invitation else {
            transaction.rollback().await?;
            return Ok(RegistrationAttempt::InvalidCode);
        };
        if !invitation.enabled
            || invitation.expires_at.is_some_and(|expiry| expiry <= now)
            || invitation
                .max_uses
                .is_some_and(|maximum| invitation.used_count >= maximum)
        {
            transaction.rollback().await?;
            return Ok(RegistrationAttempt::InvalidCode);
        }

        let user_id = Uuid::new_v4();
        let email = canonical_email(email);
        let inserted = sqlx::query(
            "INSERT INTO users \
             (id,email,display_name,role,status,password_hash,password_changed_at, \
              balance_amount,user_group_id) \
             VALUES (?1,?2,?3,'user','active',?4,?5,?6,?7)",
        )
        .bind(user_id)
        .bind(&email)
        .bind(display_name)
        .bind(password_hash)
        .bind(timestamp_text(now))
        .bind(invitation.initial_balance_amount)
        .bind(invitation.user_group_id)
        .execute(&mut *transaction)
        .await;
        let auth_version = match inserted {
            Ok(_) => {
                sqlx::query_scalar::<_, i64>("SELECT auth_version FROM users WHERE id=?1")
                    .bind(user_id)
                    .fetch_one(&mut *transaction)
                    .await?
            }
            Err(error) if registration_user_unique_violation(&error) => {
                transaction.rollback().await?;
                return Ok(RegistrationAttempt::EmailConflict);
            }
            Err(error) => return Err(error.into()),
        };

        let now_text = timestamp_text(now);
        sqlx::query(
            "UPDATE registration_invitation_codes \
             SET used_count=used_count+1,last_used_at=?2 WHERE id=?1",
        )
        .bind(invitation.id)
        .bind(&now_text)
        .execute(&mut *transaction)
        .await?;
        insert_audit(
            &mut transaction,
            AuditInsert {
                actor_user_id: Some(user_id),
                actor_role: Some("user"),
                action: "register",
                object_id: user_id,
                before_redacted: "{}".to_owned(),
                after_redacted: registration_user_audit(
                    user_id,
                    &email,
                    display_name,
                    invitation.initial_balance_amount.into_inner(),
                    invitation.user_group_id,
                    invitation.id,
                )?,
                correlation_id: Uuid::new_v4(),
            },
        )
        .await?;
        transaction.commit().await?;

        Ok(RegistrationAttempt::Registered(SessionUser {
            id: user_id,
            email: Some(email),
            display_name: display_name.to_owned(),
            role: UserRole::User,
            auth_version,
            session_purpose: ConsoleSessionPurpose::Normal,
            temporary_password_expires_at: None,
        }))
    }

    pub async fn invite_user(
        &self,
        actor_user_id: Uuid,
        input: InviteUserInput,
        invitation_id: Uuid,
        invitation_token_hash: &[u8],
        invitation_ttl: Duration,
    ) -> Result<InvitationCreated, RepositoryError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        if input.email.trim().is_empty() || input.display_name.trim().is_empty() {
            transaction.rollback().await?;
            return Err(RepositoryError::Validation);
        }
        if input.initial_balance_amount.is_sign_negative() {
            transaction.rollback().await?;
            return Err(RepositoryError::Validation);
        }
        let initial_balance_amount = normalize_numeric_24_8(input.initial_balance_amount);
        let user_group_id = input.user_group_id.unwrap_or(match input.role {
            UserRole::User => DEFAULT_USER_GROUP_ID,
            UserRole::Admin => DEFAULT_ADMIN_GROUP_ID,
        });
        ensure_user_group(&mut transaction, user_group_id).await?;
        if let Some(policy_id) = input.default_api_key_policy_id {
            let enabled =
                sqlx::query_scalar::<_, bool>("SELECT enabled FROM api_key_policies WHERE id=?1")
                    .bind(policy_id)
                    .fetch_optional(&mut *transaction)
                    .await?
                    .ok_or(RepositoryError::Validation)?;
            if !enabled {
                transaction.rollback().await?;
                return Err(RepositoryError::Validation);
            }
        }

        let user_id = Uuid::new_v4();
        let email = canonical_email(&input.email);
        sqlx::query(
            "INSERT INTO users \
             (id,email,display_name,role,status,balance_amount,user_group_id,default_api_key_policy_id) \
             VALUES (?1,?2,?3,?4,'invited',?5,?6,?7)",
        )
        .bind(user_id)
        .bind(&email)
        .bind(&input.display_name)
        .bind(input.role.as_str())
        .bind(SqliteDecimal::from(initial_balance_amount))
        .bind(user_group_id)
        .bind(input.default_api_key_policy_id)
        .execute(&mut *transaction)
        .await?;
        let updated_at = select_user_updated_at(&mut transaction, user_id).await?;
        let now = Utc::now();
        let expires_at = checked_expiry(now, invitation_ttl)?;
        sqlx::query(
            "INSERT INTO user_invitations (id,user_id,invited_by,token_hash,expires_at) \
             VALUES (?1,?2,?3,?4,?5)",
        )
        .bind(invitation_id)
        .bind(user_id)
        .bind(actor_user_id)
        .bind(invitation_token_hash)
        .bind(timestamp_text(expires_at))
        .execute(&mut *transaction)
        .await?;

        let correlation_id = Uuid::new_v4();
        insert_audit(
            &mut transaction,
            AuditInsert {
                actor_user_id: Some(actor_user_id),
                actor_role: Some("admin"),
                action: "invite",
                object_id: user_id,
                before_redacted: "{}".to_owned(),
                after_redacted: invite_user_audit(
                    user_id,
                    &email,
                    &input,
                    initial_balance_amount,
                    user_group_id,
                    updated_at,
                )?,
                correlation_id,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(InvitationCreated {
            user_id,
            invitation_id,
            expires_at,
            correlation_id,
        })
    }

    pub async fn reissue_invitation(
        &self,
        actor_user_id: Uuid,
        user_id: Uuid,
        invitation_id: Uuid,
        invitation_token_hash: &[u8],
        invitation_ttl: Duration,
    ) -> Result<InvitationCreated, RepositoryError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        let user = sqlx::query_as::<_, UserForReinvitation>(
            "SELECT id,email,display_name,role,status,password_hash,balance_amount, \
                    user_group_id,default_api_key_policy_id,updated_at \
             FROM users WHERE id=?1 AND is_system=0",
        )
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        if user.email.is_none()
            || user.password_hash.is_some()
            || !matches!(user.status.as_str(), "invited" | "suspended" | "disabled")
        {
            transaction.rollback().await?;
            return Err(RepositoryError::Validation);
        }

        let now = Utc::now();
        let now_text = timestamp_text(now);
        sqlx::query(
            "UPDATE user_invitations SET revoked_at=?2 \
             WHERE user_id=?1 AND accepted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(&now_text)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE users SET status='invited',auth_version=auth_version+1 WHERE id=?1")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        // The users trigger is also AFTER UPDATE.
        let updated_at = select_user_updated_at(&mut transaction, user_id).await?;
        sqlx::query(
            "UPDATE user_sessions SET revoked_at=?2 \
             WHERE user_id=?1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(&now_text)
        .execute(&mut *transaction)
        .await?;

        let expires_at = checked_expiry(now, invitation_ttl)?;
        sqlx::query(
            "INSERT INTO user_invitations \
             (id,user_id,invited_by,token_hash,expires_at) \
             VALUES (?1,?2,?3,?4,?5)",
        )
        .bind(invitation_id)
        .bind(user_id)
        .bind(actor_user_id)
        .bind(invitation_token_hash)
        .bind(timestamp_text(expires_at))
        .execute(&mut *transaction)
        .await?;

        let correlation_id = Uuid::new_v4();
        let before = reinvitation_before_audit(&user)?;
        let after = reinvitation_after_audit(&user, invitation_id, expires_at, updated_at)?;
        insert_audit(
            &mut transaction,
            AuditInsert {
                actor_user_id: Some(actor_user_id),
                actor_role: Some("admin"),
                action: "reinvite",
                object_id: user_id,
                before_redacted: before,
                after_redacted: after,
                correlation_id,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(InvitationCreated {
            user_id,
            invitation_id,
            expires_at,
            correlation_id,
        })
    }

    pub async fn accept_invitation(
        &self,
        invitation_id: Uuid,
        presented_token_hash: &[u8],
        password_hash: &str,
    ) -> Result<Option<SessionUser>, RepositoryError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = Utc::now();
        let invitation = sqlx::query_as::<_, InvitationForAcceptance>(
            "SELECT i.user_id,i.token_hash,i.expires_at,i.accepted_at,i.revoked_at, \
                    u.email,u.display_name,u.role,u.status \
             FROM user_invitations AS i \
             JOIN users AS u ON u.id=i.user_id \
             WHERE i.id=?1",
        )
        .bind(invitation_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(invitation) = invitation else {
            transaction.rollback().await?;
            return Ok(None);
        };
        if invitation.accepted_at.is_some()
            || invitation.revoked_at.is_some()
            || invitation.expires_at <= now
            || invitation.status != "invited"
            || !bool::from(invitation.token_hash.ct_eq(presented_token_hash))
        {
            transaction.rollback().await?;
            return Ok(None);
        }
        let role = parse_role(&invitation.role)?;
        let now_text = timestamp_text(now);
        sqlx::query(
            "UPDATE users \
             SET password_hash=?2,password_changed_at=?3,status='active',auth_version=auth_version+1, \
                 password_change_required=0,temporary_password_issued_at=NULL, \
                 temporary_password_expires_at=NULL \
             WHERE id=?1",
        )
        .bind(invitation.user_id)
        .bind(password_hash)
        .bind(&now_text)
        .execute(&mut *transaction)
        .await?;
        let auth_version =
            sqlx::query_scalar::<_, i64>("SELECT auth_version FROM users WHERE id=?1")
                .bind(invitation.user_id)
                .fetch_one(&mut *transaction)
                .await?;
        sqlx::query("UPDATE user_invitations SET accepted_at=?2 WHERE id=?1")
            .bind(invitation_id)
            .bind(&now_text)
            .execute(&mut *transaction)
            .await?;
        insert_audit(
            &mut transaction,
            AuditInsert {
                actor_user_id: Some(invitation.user_id),
                actor_role: Some(&invitation.role),
                action: "activate",
                object_id: invitation.user_id,
                before_redacted: "{}".to_owned(),
                after_redacted: serde_json::to_string(&ActivationAudit {
                    status: "active",
                    auth_version,
                })
                .map_err(|_| RepositoryError::Validation)?,
                correlation_id: Uuid::new_v4(),
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(Some(SessionUser {
            id: invitation.user_id,
            email: invitation.email,
            display_name: invitation.display_name,
            role,
            auth_version,
            session_purpose: ConsoleSessionPurpose::Normal,
            temporary_password_expires_at: None,
        }))
    }
}

async fn select_registration_invitation_code<'e, E>(
    executor: E,
    id: Uuid,
) -> Result<Option<RegistrationInvitationCode>, RepositoryError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    sqlx::query_as::<_, RegistrationInvitationCode>(
        "SELECT id,name,max_uses,used_count,expires_at,enabled,user_group_id, \
                initial_balance_amount,created_by,last_used_at,created_at,updated_at \
         FROM registration_invitation_codes WHERE id=?1",
    )
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(RepositoryError::from)
}

async fn registration_invitation_code_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    id: Uuid,
) -> Result<RegistrationInvitationCode, RepositoryError> {
    select_registration_invitation_code(&mut **transaction, id)
        .await?
        .ok_or(RepositoryError::NotFound)
}

async fn ensure_active_admin(
    transaction: &mut Transaction<'_, Sqlite>,
    user_id: Uuid,
) -> Result<(), RepositoryError> {
    let admin = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND status='active' AND role='admin')",
    )
    .bind(user_id)
    .fetch_one(&mut **transaction)
    .await?;
    if admin {
        Ok(())
    } else {
        Err(RepositoryError::NotFound)
    }
}

async fn ensure_registration_user_group(
    transaction: &mut Transaction<'_, Sqlite>,
    user_group_id: Uuid,
) -> Result<(), RepositoryError> {
    ensure_user_group(transaction, user_group_id).await
}

async fn ensure_user_group(
    transaction: &mut Transaction<'_, Sqlite>,
    user_group_id: Uuid,
) -> Result<(), RepositoryError> {
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM user_groups WHERE id=?1)")
            .bind(user_group_id)
            .fetch_one(&mut **transaction)
            .await?;
    if exists {
        Ok(())
    } else {
        Err(RepositoryError::Validation)
    }
}

fn validate_registration_invitation_code_input(
    input: &RegistrationInvitationCodeInput,
    used_count: i64,
) -> Result<(), RepositoryError> {
    if input.name.trim().is_empty()
        || input.name.len() > 100
        || input.max_uses.is_some_and(|maximum| maximum <= 0)
        || input.max_uses.is_some_and(|maximum| maximum < used_count)
        || input.initial_balance_amount.is_sign_negative()
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn registration_invitation_code_audit(
    code: &RegistrationInvitationCode,
) -> Result<String, RepositoryError> {
    serde_json::to_string(&RegistrationCodeAudit {
        id: code.id,
        name: &code.name,
        max_uses: code.max_uses,
        used_count: code.used_count,
        expires_at: code.expires_at,
        enabled: code.enabled,
        user_group_id: code.user_group_id,
        initial_balance_amount: code.initial_balance_amount,
        created_by: code.created_by,
        last_used_at: code.last_used_at,
        created_at: code.created_at,
        updated_at: code.updated_at,
    })
    .map_err(|_| RepositoryError::Validation)
}

async fn insert_registration_invitation_code_audit(
    transaction: &mut Transaction<'_, Sqlite>,
    actor_user_id: Uuid,
    action: &str,
    id: Uuid,
    before_redacted: String,
    after_redacted: String,
    correlation_id: Uuid,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO audit_logs \
         (id,actor_user_id,actor_type,actor_role,action,object_type,object_id, \
          before_redacted,after_redacted,correlation_id) \
         VALUES (?1,?2,'user','admin',?3,'registration_invitation_code',?4,?5,?6,?7)",
    )
    .bind(Uuid::new_v4())
    .bind(actor_user_id)
    .bind(action)
    .bind(id)
    .bind(before_redacted)
    .bind(after_redacted)
    .bind(correlation_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

struct AuditInsert<'a> {
    actor_user_id: Option<Uuid>,
    actor_role: Option<&'a str>,
    action: &'a str,
    object_id: Uuid,
    before_redacted: String,
    after_redacted: String,
    correlation_id: Uuid,
}

async fn insert_audit(
    transaction: &mut Transaction<'_, Sqlite>,
    input: AuditInsert<'_>,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO audit_logs \
         (id,actor_user_id,actor_type,actor_role,action,object_type,object_id, \
          before_redacted,after_redacted,correlation_id) \
         VALUES (?1,?2,'user',?3,?4,'user',?5,?6,?7,?8)",
    )
    .bind(Uuid::new_v4())
    .bind(input.actor_user_id)
    .bind(input.actor_role)
    .bind(input.action)
    .bind(input.object_id)
    .bind(input.before_redacted)
    .bind(input.after_redacted)
    .bind(input.correlation_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn select_user_updated_at(
    transaction: &mut Transaction<'_, Sqlite>,
    user_id: Uuid,
) -> Result<DateTime<Utc>, RepositoryError> {
    sqlx::query_scalar("SELECT updated_at FROM users WHERE id=?1")
        .bind(user_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(RepositoryError::from)
}

fn checked_expiry(now: DateTime<Utc>, ttl: Duration) -> Result<DateTime<Utc>, RepositoryError> {
    now.checked_add_signed(
        chrono::Duration::from_std(ttl).map_err(|_| RepositoryError::Validation)?,
    )
    .ok_or(RepositoryError::Validation)
}

fn registration_code_unique_violation(error: &sqlx::Error) -> bool {
    sqlite_unique_violation(error)
        && error.as_database_error().is_some_and(|database| {
            matches!(
                database.message(),
                "UNIQUE constraint failed: registration_invitation_codes.name"
                    | "UNIQUE constraint failed: registration_invitation_codes.code_hash"
            )
        })
}

fn registration_user_unique_violation(error: &sqlx::Error) -> bool {
    sqlite_unique_violation(error)
        && error.as_database_error().is_some_and(|database| {
            matches!(
                database.message(),
                "UNIQUE constraint failed: users.display_name"
                    | "UNIQUE constraint failed: index 'users_email_lower_unique_idx'"
            )
        })
}

fn sqlite_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "2067")
}

fn etag_timestamp_text(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn registration_user_audit(
    id: Uuid,
    email: &str,
    display_name: &str,
    balance_amount: rust_decimal::Decimal,
    user_group_id: Uuid,
    registration_invitation_code_id: Uuid,
) -> Result<String, RepositoryError> {
    serde_json::to_string(&RegistrationUserAudit {
        id,
        email,
        display_name,
        role: "user",
        status: "active",
        balance_amount,
        user_group_id,
        registration_invitation_code_id,
    })
    .map_err(|_| RepositoryError::Validation)
}

fn invite_user_audit(
    id: Uuid,
    email: &str,
    input: &InviteUserInput,
    balance_amount: rust_decimal::Decimal,
    user_group_id: Uuid,
    updated_at: DateTime<Utc>,
) -> Result<String, RepositoryError> {
    serde_json::to_string(&InvitedUserAudit {
        id,
        email,
        display_name: &input.display_name,
        role: input.role.as_str(),
        status: "invited",
        balance_amount,
        user_group_id,
        default_api_key_policy_id: input.default_api_key_policy_id,
        updated_at,
    })
    .map_err(|_| RepositoryError::Validation)
}

fn reinvitation_before_audit(user: &UserForReinvitation) -> Result<String, RepositoryError> {
    serde_json::to_string(&ReinvitationBeforeAudit {
        id: user.id,
        email: user.email.as_deref(),
        display_name: &user.display_name,
        role: &user.role,
        status: &user.status,
        balance_amount: user.balance_amount.into_inner(),
        user_group_id: user.user_group_id,
        default_api_key_policy_id: user.default_api_key_policy_id,
        updated_at: user.updated_at,
    })
    .map_err(|_| RepositoryError::Validation)
}

fn reinvitation_after_audit(
    user: &UserForReinvitation,
    invitation_id: Uuid,
    invitation_expires_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> Result<String, RepositoryError> {
    serde_json::to_string(&ReinvitationAfterAudit {
        id: user.id,
        email: user.email.as_deref(),
        display_name: &user.display_name,
        role: &user.role,
        status: "invited",
        balance_amount: user.balance_amount.into_inner(),
        user_group_id: user.user_group_id,
        default_api_key_policy_id: user.default_api_key_policy_id,
        invitation_id,
        invitation_expires_at,
        updated_at,
    })
    .map_err(|_| RepositoryError::Validation)
}

#[derive(FromRow)]
struct RegistrationInvitationCodeForUse {
    id: Uuid,
    max_uses: Option<i64>,
    used_count: i64,
    expires_at: Option<DateTime<Utc>>,
    enabled: bool,
    user_group_id: Uuid,
    initial_balance_amount: SqliteDecimal,
}

#[derive(FromRow)]
struct InvitationForAcceptance {
    user_id: Uuid,
    token_hash: Vec<u8>,
    expires_at: DateTime<Utc>,
    accepted_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
}

#[derive(FromRow)]
struct UserForReinvitation {
    id: Uuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    password_hash: Option<String>,
    balance_amount: SqliteDecimal,
    user_group_id: Uuid,
    default_api_key_policy_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct RegistrationCodeAudit<'a> {
    id: Uuid,
    name: &'a str,
    max_uses: Option<i64>,
    used_count: i64,
    expires_at: Option<DateTime<Utc>>,
    enabled: bool,
    user_group_id: Uuid,
    initial_balance_amount: rust_decimal::Decimal,
    created_by: Uuid,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct RegistrationUserAudit<'a> {
    id: Uuid,
    email: &'a str,
    display_name: &'a str,
    role: &'static str,
    status: &'static str,
    balance_amount: rust_decimal::Decimal,
    user_group_id: Uuid,
    registration_invitation_code_id: Uuid,
}

#[derive(Serialize)]
struct InvitedUserAudit<'a> {
    id: Uuid,
    email: &'a str,
    display_name: &'a str,
    role: &'a str,
    status: &'static str,
    balance_amount: rust_decimal::Decimal,
    user_group_id: Uuid,
    default_api_key_policy_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct ReinvitationBeforeAudit<'a> {
    id: Uuid,
    email: Option<&'a str>,
    display_name: &'a str,
    role: &'a str,
    status: &'a str,
    balance_amount: rust_decimal::Decimal,
    user_group_id: Uuid,
    default_api_key_policy_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct ReinvitationAfterAudit<'a> {
    id: Uuid,
    email: Option<&'a str>,
    display_name: &'a str,
    role: &'a str,
    status: &'static str,
    balance_amount: rust_decimal::Decimal,
    user_group_id: Uuid,
    default_api_key_policy_id: Option<Uuid>,
    invitation_id: Uuid,
    invitation_expires_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct ActivationAudit {
    status: &'static str,
    auth_version: i64,
}
