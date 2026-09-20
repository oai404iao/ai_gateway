//! SQLite persistence for Console identities, sessions, invitations, and registration codes.
//!
//! Writes run on the single shared `BEGIN IMMEDIATE` writer; updates to tables covered by the S2
//! timestamp triggers set `updated_at=ag_now()` explicitly, and transaction time is read back with
//! `ag_now()` so in-transaction expiry checks and `CHECK` constraints share one clock.

use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;
use serde_json::value::RawValue;
use sqlx::{FromRow, Sqlite, Transaction, pool::PoolConnection};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    domain::{ConsoleSessionPurpose, UserRole},
    persistence::{
        ConsoleProfile, ConsoleSession, ConsoleSessionState, DEFAULT_ADMIN_GROUP_ID,
        DEFAULT_USER_GROUP_ID, InvitationCreated, InviteUserInput, LiveConsoleIdentity, LoginUser,
        PasswordUser, RegistrationAttempt, RegistrationInvitationCode,
        RegistrationInvitationCodeInput, RegistrationInvitationCodeMutation, RepositoryError,
        SessionRotation, SessionUser, TemporaryPasswordCreated,
    },
};

use super::{SqliteAmount, SqliteDatabase, SqliteOpenError, SqliteTimestamp, SqliteUuid};

/// SQLite implementation of the Console identity, session, invitation, and
/// registration-code operations. The parent backend facade dispatches to this repository; it is
/// also directly constructible for backend contract tests.
#[derive(Clone)]
pub struct SqliteAuthRepository {
    database: Arc<SqliteDatabase>,
}

impl SqliteAuthRepository {
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    pub async fn find_login_user(&self, email: &str) -> Result<Option<LoginUser>, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, LoginUserRow>(
            "SELECT id,email,display_name,role,status,password_hash,auth_version, \
                    password_change_required,temporary_password_expires_at \
             FROM users WHERE ag_lower(email)=ag_lower(?)",
        )
        .bind(email)
        .fetch_optional(&mut *reader)
        .await?;
        Ok(row.map(LoginUserRow::into_login_user))
    }

    pub async fn validate_console_identity(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        auth_version: i64,
    ) -> Result<Option<LiveConsoleIdentity>, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, IdentityRow>(
            "SELECT u.id AS user_id,u.email,u.display_name,u.role,u.status,u.auth_version, \
                    u.password_change_required,u.temporary_password_expires_at, \
                    s.id AS session_id,s.expires_at,s.revoked_at,s.purpose AS session_purpose \
             FROM users AS u \
             JOIN user_sessions AS s ON s.user_id=u.id \
             WHERE u.id=? AND s.id=? AND u.auth_version=?",
        )
        .bind(SqliteUuid(user_id))
        .bind(SqliteUuid(session_id))
        .bind(auth_version)
        .fetch_optional(&mut *reader)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let now = Utc::now();
        let purpose = ConsoleSessionPurpose::parse(&row.session_purpose);
        let temporary_password_valid = row
            .temporary_password_expires_at
            .is_some_and(|expiry| expiry.0 > now);
        if row.status != "active"
            || row.revoked_at.is_some()
            || row.expires_at.0 <= now
            || match purpose {
                Some(ConsoleSessionPurpose::Normal) => row.password_change_required,
                Some(ConsoleSessionPurpose::PasswordChange) => {
                    !row.password_change_required || !temporary_password_valid
                }
                None => true,
            }
        {
            return Ok(None);
        }
        Ok(Some(LiveConsoleIdentity {
            user_id: row.user_id.0,
            email: row.email,
            display_name: row.display_name,
            role: row.role,
            status: row.status,
            auth_version: row.auth_version,
            session_id: row.session_id.0,
            expires_at: row.expires_at.0,
            revoked_at: row.revoked_at.map(|value| value.0),
            session_purpose: row.session_purpose,
        }))
    }

    pub async fn create_session(
        &self,
        id: Uuid,
        user_id: Uuid,
        refresh_token_hash: &[u8],
        expires_at: DateTime<Utc>,
        user_agent: Option<&str>,
        purpose: ConsoleSessionPurpose,
    ) -> Result<(), RepositoryError> {
        let mut transaction = self.write().await?;
        sqlx::query(
            "INSERT INTO user_sessions \
             (id,user_id,refresh_token_hash,expires_at,user_agent,purpose) \
             VALUES (?,?,?,?,?,?)",
        )
        .bind(SqliteUuid(id))
        .bind(SqliteUuid(user_id))
        .bind(refresh_token_hash)
        .bind(SqliteTimestamp(expires_at))
        .bind(user_agent)
        .bind(purpose.as_str())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn password_user(
        &self,
        user_id: Uuid,
    ) -> Result<Option<PasswordUser>, RepositoryError> {
        let mut reader = self.read().await?;
        let row = sqlx::query_as::<_, PasswordUserRow>(
            "SELECT id,password_hash,status,role,auth_version,password_change_required, \
                    temporary_password_expires_at \
             FROM users WHERE id=?",
        )
        .bind(SqliteUuid(user_id))
        .fetch_optional(&mut *reader)
        .await?;
        Ok(row.map(PasswordUserRow::into_password_user))
    }

    /// Rotates a refresh credential under the immediate write lock. A mismatched credential for an
    /// otherwise active session is replay and revokes that session, as in PostgreSQL.
    pub async fn rotate_session(
        &self,
        session_id: Uuid,
        presented_hash: &[u8],
        next_hash: &[u8],
        next_expires_at: DateTime<Utc>,
        user_agent: Option<&str>,
    ) -> Result<SessionRotation, RepositoryError> {
        let mut transaction = self.write().await?;
        let now = transaction_now(&mut transaction).await?;
        let row = sqlx::query_as::<_, SessionForRotationRow>(
            "SELECT s.user_id,s.refresh_token_hash,s.expires_at,s.revoked_at,s.purpose, \
                    u.email,u.display_name,u.role,u.status,u.auth_version, \
                    u.password_change_required,u.temporary_password_expires_at \
             FROM user_sessions AS s \
             JOIN users AS u ON u.id=s.user_id \
             WHERE s.id=?",
        )
        .bind(SqliteUuid(session_id))
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(session) = row else {
            transaction.rollback().await?;
            return Ok(SessionRotation::Invalid);
        };
        let purpose = parse_session_purpose(&session.purpose)?;
        if session.revoked_at.is_some()
            || session.expires_at.0 <= now
            || session.status != "active"
            || match purpose {
                ConsoleSessionPurpose::Normal => session.password_change_required,
                ConsoleSessionPurpose::PasswordChange => {
                    !session.password_change_required
                        || session
                            .temporary_password_expires_at
                            .is_none_or(|expiry| expiry.0 <= now)
                }
            }
        {
            transaction.rollback().await?;
            return Ok(SessionRotation::Invalid);
        }
        if !bool::from(session.refresh_token_hash.ct_eq(presented_hash)) {
            sqlx::query(
                "UPDATE user_sessions SET revoked_at=ag_now() WHERE id=? AND revoked_at IS NULL",
            )
            .bind(SqliteUuid(session_id))
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Ok(SessionRotation::Replayed);
        }
        let next_expires_at = match purpose {
            ConsoleSessionPurpose::Normal => next_expires_at,
            ConsoleSessionPurpose::PasswordChange => next_expires_at.min(
                session
                    .temporary_password_expires_at
                    .ok_or(RepositoryError::Validation)?
                    .0,
            ),
        };
        sqlx::query(
            "UPDATE user_sessions \
             SET refresh_token_hash=?,expires_at=?,rotated_at=ag_now(),last_seen_at=ag_now(), \
                 user_agent=COALESCE(?,user_agent) \
             WHERE id=?",
        )
        .bind(next_hash)
        .bind(SqliteTimestamp(next_expires_at))
        .bind(user_agent)
        .bind(SqliteUuid(session_id))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(SessionRotation::Rotated {
            user: SessionUser {
                id: session.user_id.0,
                email: session.email,
                display_name: session.display_name,
                role: parse_role(&session.role)?,
                auth_version: session.auth_version,
                session_purpose: purpose,
                temporary_password_expires_at: session
                    .temporary_password_expires_at
                    .map(|value| value.0),
            },
            refresh_expires_at: next_expires_at,
        })
    }

    pub async fn revoke_session_for_user(
        &self,
        user_id: Uuid,
        session_id: Uuid,
    ) -> Result<bool, RepositoryError> {
        let mut transaction = self.write().await?;
        let result = sqlx::query(
            "UPDATE user_sessions SET revoked_at=ag_now() \
             WHERE id=? AND user_id=? AND revoked_at IS NULL",
        )
        .bind(SqliteUuid(session_id))
        .bind(SqliteUuid(user_id))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn revoke_all_sessions(&self, user_id: Uuid) -> Result<(), RepositoryError> {
        let mut transaction = self.write().await?;
        revoke_live_sessions(&mut transaction, user_id).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_other_sessions(
        &self,
        user_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<u64, RepositoryError> {
        let mut transaction = self.write().await?;
        let now = transaction_now(&mut transaction).await?;
        let result = sqlx::query(
            "UPDATE user_sessions SET revoked_at=ag_now() \
             WHERE user_id=? AND id<>? AND revoked_at IS NULL AND expires_at>?",
        )
        .bind(SqliteUuid(user_id))
        .bind(SqliteUuid(current_session_id))
        .bind(SqliteTimestamp(now))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn sessions_for_user(
        &self,
        user_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<Vec<ConsoleSession>, RepositoryError> {
        let mut reader = self.read().await?;
        let rows = sqlx::query_as::<_, SessionRow>(
            "SELECT id,user_agent,created_at,last_seen_at,expires_at,revoked_at \
             FROM user_sessions WHERE user_id=? \
             ORDER BY (id=?) DESC, \
                      (revoked_at IS NULL AND expires_at>?) DESC, \
                      created_at DESC,id",
        )
        .bind(SqliteUuid(user_id))
        .bind(SqliteUuid(current_session_id))
        .bind(SqliteTimestamp(Utc::now()))
        .fetch_all(&mut *reader)
        .await?;
        let now = Utc::now();
        Ok(rows
            .into_iter()
            .map(|session| ConsoleSession {
                state: if session.revoked_at.is_some() {
                    ConsoleSessionState::Revoked
                } else if session.expires_at.0 <= now {
                    ConsoleSessionState::Expired
                } else {
                    ConsoleSessionState::Active
                },
                id: session.id.0,
                user_agent: session.user_agent,
                created_at: session.created_at.0,
                last_seen_at: session.last_seen_at.0,
                expires_at: session.expires_at.0,
                revoked_at: session.revoked_at.map(|value| value.0),
                is_current: session.id.0 == current_session_id,
            })
            .collect())
    }

    pub async fn profile(&self, user_id: Uuid) -> Result<Option<ConsoleProfile>, RepositoryError> {
        let mut reader = self.read().await?;
        select_profile(&mut *reader, user_id).await
    }

    pub async fn update_display_name(
        &self,
        user_id: Uuid,
        display_name: &str,
    ) -> Result<Option<ConsoleProfile>, RepositoryError> {
        let mut transaction = self.write().await?;
        let profile = sqlx::query_as::<_, ProfileRow>(
            "UPDATE users SET display_name=?,updated_at=ag_now() \
             WHERE id=? AND status='active' AND deleted_at IS NULL \
             RETURNING id,email,display_name,role,status,balance_amount,created_at,updated_at",
        )
        .bind(display_name)
        .bind(SqliteUuid(user_id))
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(profile) = profile else {
            transaction.rollback().await?;
            return Ok(None);
        };
        transaction.commit().await?;
        Ok(Some(profile.into_profile()))
    }

    pub async fn registration_invitation_codes(
        &self,
    ) -> Result<Vec<RegistrationInvitationCode>, RepositoryError> {
        let mut reader = self.read().await?;
        let rows = sqlx::query_as::<_, InvitationCodeRow>(
            "SELECT id,name,max_uses,used_count,expires_at,enabled,user_group_id, \
                    initial_balance_amount,created_by,last_used_at,created_at,updated_at \
             FROM registration_invitation_codes ORDER BY created_at DESC,id",
        )
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.into_iter().map(InvitationCodeRow::into_code).collect())
    }

    pub async fn registration_invitation_code(
        &self,
        id: Uuid,
    ) -> Result<Option<RegistrationInvitationCode>, RepositoryError> {
        let mut reader = self.read().await?;
        select_registration_invitation_code(&mut *reader, id).await
    }

    pub async fn change_password(
        &self,
        user_id: Uuid,
        password_hash: &str,
    ) -> Result<bool, RepositoryError> {
        let mut transaction = self.write().await?;
        let changed = sqlx::query(
            "UPDATE users \
             SET password_hash=?,password_changed_at=ag_now(),auth_version=auth_version+1, \
                 password_change_required=0,temporary_password_issued_at=NULL, \
                 temporary_password_expires_at=NULL,updated_at=ag_now() \
             WHERE id=? AND status='active' AND password_change_required=0 \
               AND deleted_at IS NULL",
        )
        .bind(password_hash)
        .bind(SqliteUuid(user_id))
        .execute(&mut *transaction)
        .await?;
        if changed.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        revoke_live_sessions(&mut transaction, user_id).await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Replaces an active user's Console password with an expiring, administrator-issued temporary
    /// password. The plaintext is produced by the application layer and never reaches persistence.
    pub async fn issue_temporary_password(
        &self,
        actor_user_id: Uuid,
        actor_auth_version: i64,
        user_id: Uuid,
        password_hash: &str,
        temporary_password_ttl: Duration,
    ) -> Result<TemporaryPasswordCreated, RepositoryError> {
        if actor_user_id == user_id {
            return Err(RepositoryError::CannotResetSelf);
        }
        let mut transaction = self.write().await?;
        let actor_is_current_admin = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS( \
                SELECT 1 FROM users \
                WHERE id=? AND auth_version=? AND status='active' AND role='admin' \
                  AND password_change_required=0 AND deleted_at IS NULL \
             )",
        )
        .bind(SqliteUuid(actor_user_id))
        .bind(actor_auth_version)
        .fetch_one(&mut *transaction)
        .await?;
        if !actor_is_current_admin {
            transaction.rollback().await?;
            return Err(RepositoryError::NotFound);
        }

        let target = sqlx::query_as::<_, TemporaryPasswordTargetRow>(
            "SELECT status,(password_hash IS NOT NULL) AS has_password,is_system,deleted_at \
             FROM users WHERE id=?",
        )
        .bind(SqliteUuid(user_id))
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(target) = target else {
            transaction.rollback().await?;
            return Err(RepositoryError::NotFound);
        };
        if target.is_system || target.deleted_at.is_some() {
            transaction.rollback().await?;
            return Err(RepositoryError::NotFound);
        }
        if target.status != "active" || !target.has_password {
            transaction.rollback().await?;
            return Err(RepositoryError::TemporaryPasswordUnavailable);
        }

        let before = user_audit(&mut transaction, user_id).await?;
        let now = transaction_now(&mut transaction).await?;
        let expires_at = now
            .checked_add_signed(
                chrono::Duration::from_std(temporary_password_ttl)
                    .map_err(|_| RepositoryError::Validation)?,
            )
            .ok_or(RepositoryError::Validation)?;
        sqlx::query(
            "UPDATE users SET \
             password_hash=?,password_change_required=1, \
             temporary_password_issued_at=?,temporary_password_expires_at=?, \
             auth_version=auth_version+1,updated_at=ag_now() \
             WHERE id=?",
        )
        .bind(password_hash)
        .bind(SqliteTimestamp(now))
        .bind(SqliteTimestamp(expires_at))
        .bind(SqliteUuid(user_id))
        .execute(&mut *transaction)
        .await?;
        revoke_live_sessions(&mut transaction, user_id).await?;

        let correlation_id = Uuid::new_v4();
        let after = user_audit(&mut transaction, user_id).await?;
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: Some(actor_user_id),
                actor_type: "user",
                actor_role: Some("admin"),
                action: "issue_temporary_password",
                object_type: "user",
                object_id: user_id,
                before_redacted: before,
                after_redacted: after,
                correlation_id,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(TemporaryPasswordCreated {
            user_id,
            expires_at,
            correlation_id,
        })
    }

    /// Replaces a temporary password with the user's chosen permanent password and revokes every
    /// password-change session.
    pub async fn complete_temporary_password(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        expected_auth_version: i64,
        password_hash: &str,
    ) -> Result<Option<SessionUser>, RepositoryError> {
        let mut transaction = self.write().await?;
        let now = transaction_now(&mut transaction).await?;
        let user = sqlx::query_as::<_, TemporaryPasswordCompletionRow>(
            "SELECT u.email,u.display_name,u.role,u.status,u.auth_version, \
                    u.password_change_required,u.temporary_password_expires_at, \
                    s.purpose AS session_purpose,s.expires_at AS session_expires_at, \
                    s.revoked_at AS session_revoked_at \
             FROM users AS u \
             JOIN user_sessions AS s ON s.user_id=u.id \
             WHERE u.id=? AND s.id=?",
        )
        .bind(SqliteUuid(user_id))
        .bind(SqliteUuid(session_id))
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(user) = user else {
            transaction.rollback().await?;
            return Ok(None);
        };
        if user.status != "active"
            || user.auth_version != expected_auth_version
            || !user.password_change_required
            || user
                .temporary_password_expires_at
                .is_none_or(|expiry| expiry.0 <= now)
            || parse_session_purpose(&user.session_purpose)?
                != ConsoleSessionPurpose::PasswordChange
            || user.session_revoked_at.is_some()
            || user.session_expires_at.0 <= now
        {
            transaction.rollback().await?;
            return Ok(None);
        }

        let before = user_audit(&mut transaction, user_id).await?;
        let auth_version = sqlx::query_scalar::<_, i64>(
            "UPDATE users SET \
             password_hash=?,password_changed_at=ag_now(),auth_version=auth_version+1, \
             password_change_required=0,temporary_password_issued_at=NULL, \
             temporary_password_expires_at=NULL,updated_at=ag_now() \
             WHERE id=? AND auth_version=? RETURNING auth_version",
        )
        .bind(password_hash)
        .bind(SqliteUuid(user_id))
        .bind(expected_auth_version)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(auth_version) = auth_version else {
            transaction.rollback().await?;
            return Ok(None);
        };
        revoke_live_sessions(&mut transaction, user_id).await?;
        let correlation_id = Uuid::new_v4();
        let after = user_audit(&mut transaction, user_id).await?;
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: Some(user_id),
                actor_type: "user",
                actor_role: Some(&user.role),
                action: "complete_password_reset",
                object_type: "user",
                object_id: user_id,
                before_redacted: before,
                after_redacted: after,
                correlation_id,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(Some(SessionUser {
            id: user_id,
            email: user.email,
            display_name: user.display_name,
            role: parse_role(&user.role)?,
            auth_version,
            session_purpose: ConsoleSessionPurpose::Normal,
            temporary_password_expires_at: None,
        }))
    }

    /// Emergency operator recovery for an active Console administrator. Every existing session is
    /// revoked, exactly as for a self-service password change.
    pub async fn reset_active_admin_password(
        &self,
        email: &str,
        password_hash: &str,
    ) -> Result<bool, RepositoryError> {
        let mut transaction = self.write().await?;
        let admin = sqlx::query_as::<_, PasswordResetAdminRow>(
            "UPDATE users \
             SET password_hash=?,password_changed_at=ag_now(),auth_version=auth_version+1, \
                 password_change_required=0,temporary_password_issued_at=NULL, \
                 temporary_password_expires_at=NULL,updated_at=ag_now() \
             WHERE ag_lower(email)=ag_lower(?) AND role='admin' AND status='active' \
               AND deleted_at IS NULL \
             RETURNING id,email,auth_version",
        )
        .bind(password_hash)
        .bind(email)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(admin) = admin else {
            transaction.rollback().await?;
            return Ok(false);
        };
        revoke_live_sessions(&mut transaction, admin.id.0).await?;
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: None,
                actor_type: "system",
                actor_role: None,
                action: "reset_password",
                object_type: "user",
                object_id: admin.id.0,
                before_redacted: to_json_text(&ResetAdminBeforeAudit {
                    email: admin.email.as_deref(),
                    role: "admin",
                    status: "active",
                })?,
                after_redacted: to_json_text(&ResetAdminAfterAudit {
                    email: admin.email.as_deref(),
                    role: "admin",
                    status: "active",
                    auth_version: admin.auth_version,
                    password_changed: true,
                })?,
                correlation_id: Uuid::new_v4(),
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn create_registration_invitation_code(
        &self,
        actor_user_id: Uuid,
        code_hash: &[u8],
        input: RegistrationInvitationCodeInput,
    ) -> Result<RegistrationInvitationCodeMutation, RepositoryError> {
        let mut transaction = self.write().await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        validate_registration_invitation_code_input(&input, 0)?;
        ensure_user_group(&mut transaction, input.user_group_id).await?;
        let initial_balance_amount = amount_24_8(input.initial_balance_amount)?;

        let id = Uuid::new_v4();
        let inserted = sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO registration_invitation_codes \
             (id,name,code_hash,max_uses,expires_at,enabled,user_group_id, \
              initial_balance_amount,created_by) \
             VALUES (?,?,?,?,?,?,?,?,?) \
             ON CONFLICT DO NOTHING RETURNING updated_at",
        )
        .bind(SqliteUuid(id))
        .bind(input.name.trim())
        .bind(code_hash)
        .bind(input.max_uses)
        .bind(input.expires_at.map(SqliteTimestamp))
        .bind(input.enabled)
        .bind(SqliteUuid(input.user_group_id))
        .bind(initial_balance_amount)
        .bind(SqliteUuid(actor_user_id))
        .fetch_optional(&mut *transaction)
        .await?;
        if inserted.is_none() {
            transaction.rollback().await?;
            return Err(RepositoryError::RegistrationInvitationCodeConflict);
        }

        let after = registration_invitation_code_in_transaction(&mut transaction, id).await?;
        let correlation_id = Uuid::new_v4();
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: Some(actor_user_id),
                actor_type: "user",
                actor_role: Some("admin"),
                action: "create",
                object_type: "registration_invitation_code",
                object_id: id,
                before_redacted: "{}".to_owned(),
                after_redacted: registration_invitation_code_audit(&after)?,
                correlation_id,
            },
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
        let mut transaction = self.write().await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        let before = registration_invitation_code_in_transaction(&mut transaction, id).await?;
        if before.updated_at != expected_updated_at {
            transaction.rollback().await?;
            return Err(RepositoryError::Conflict);
        }
        validate_registration_invitation_code_input(&input, before.used_count)?;
        ensure_user_group(&mut transaction, input.user_group_id).await?;
        if registration_code_name_taken(&mut transaction, input.name.trim(), id).await? {
            transaction.rollback().await?;
            return Err(RepositoryError::RegistrationInvitationCodeConflict);
        }
        let initial_balance_amount = amount_24_8(input.initial_balance_amount)?;

        let updated = sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE registration_invitation_codes SET \
             name=?,max_uses=?,expires_at=?,enabled=?,user_group_id=?, \
             initial_balance_amount=?,updated_at=ag_now() \
             WHERE id=? AND updated_at=? RETURNING updated_at",
        )
        .bind(input.name.trim())
        .bind(input.max_uses)
        .bind(input.expires_at.map(SqliteTimestamp))
        .bind(input.enabled)
        .bind(SqliteUuid(input.user_group_id))
        .bind(initial_balance_amount)
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected_updated_at))
        .fetch_optional(&mut *transaction)
        .await?;
        if updated.is_none() {
            transaction.rollback().await?;
            return Err(RepositoryError::Conflict);
        }

        // The S2 timestamp guard is BEFORE UPDATE, so `ag_now()` is written by this statement and
        // the post-update row is re-read for the audit payload.
        let after = registration_invitation_code_in_transaction(&mut transaction, id).await?;
        let correlation_id = Uuid::new_v4();
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: Some(actor_user_id),
                actor_type: "user",
                actor_role: Some("admin"),
                action: "update",
                object_type: "registration_invitation_code",
                object_id: id,
                before_redacted: registration_invitation_code_audit(&before)?,
                after_redacted: registration_invitation_code_audit(&after)?,
                correlation_id,
            },
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
        let mut transaction = self.write().await?;
        let now = transaction_now(&mut transaction).await?;
        let invitation = sqlx::query_as::<_, InvitationCodeForUseRow>(
            "SELECT id,max_uses,used_count,expires_at,enabled,user_group_id, \
                    initial_balance_amount \
             FROM registration_invitation_codes AS code \
             WHERE code.code_hash=? \
               AND EXISTS( \
                   SELECT 1 FROM user_groups AS user_group \
                   WHERE user_group.id=code.user_group_id AND user_group.deleted_at IS NULL \
               )",
        )
        .bind(code_hash)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(invitation) = invitation else {
            transaction.rollback().await?;
            return Ok(RegistrationAttempt::InvalidCode);
        };
        if !invitation.enabled
            || invitation.expires_at.is_some_and(|expiry| expiry.0 <= now)
            || invitation
                .max_uses
                .is_some_and(|maximum| invitation.used_count >= maximum)
        {
            transaction.rollback().await?;
            return Ok(RegistrationAttempt::InvalidCode);
        }

        let user_id = Uuid::new_v4();
        let email = email.to_owned();
        let auth_version = sqlx::query_scalar::<_, i64>(
            "INSERT INTO users \
             (id,email,display_name,role,status,password_hash,password_changed_at, \
              balance_amount,user_group_id) \
             VALUES (?,?,?,'user','active',?,ag_now(),?,?) \
             ON CONFLICT DO NOTHING RETURNING auth_version",
        )
        .bind(SqliteUuid(user_id))
        .bind(&email)
        .bind(display_name)
        .bind(password_hash)
        .bind(invitation.initial_balance_amount)
        .bind(invitation.user_group_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(auth_version) = auth_version else {
            transaction.rollback().await?;
            return Ok(RegistrationAttempt::EmailConflict);
        };

        sqlx::query(
            "UPDATE registration_invitation_codes \
             SET used_count=used_count+1,last_used_at=ag_now(),updated_at=ag_now() WHERE id=?",
        )
        .bind(SqliteUuid(invitation.id.0))
        .execute(&mut *transaction)
        .await?;
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: Some(user_id),
                actor_type: "user",
                actor_role: Some("user"),
                action: "register",
                object_type: "user",
                object_id: user_id,
                before_redacted: "{}".to_owned(),
                after_redacted: to_json_text(&RegisteredUserAudit {
                    id: user_id,
                    email: &email,
                    display_name,
                    role: "user",
                    status: "active",
                    balance_amount: invitation.initial_balance_amount.0,
                    user_group_id: invitation.user_group_id.0,
                    registration_invitation_code_id: invitation.id.0,
                })?,
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
        let mut transaction = self.write().await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        if input.email.trim().is_empty() || input.display_name.trim().is_empty() {
            transaction.rollback().await?;
            return Err(RepositoryError::Validation);
        }
        if input.initial_balance_amount.is_sign_negative() {
            transaction.rollback().await?;
            return Err(RepositoryError::Validation);
        }
        let stored_balance_amount = normalize_numeric_24_8(input.initial_balance_amount);
        let user_group_id = input.user_group_id.unwrap_or(match input.role {
            UserRole::User => DEFAULT_USER_GROUP_ID,
            UserRole::Admin => DEFAULT_ADMIN_GROUP_ID,
        });
        ensure_user_group(&mut transaction, user_group_id).await?;
        if let Some(policy_id) = input.default_api_key_policy_id {
            let enabled =
                sqlx::query_scalar::<_, bool>("SELECT enabled FROM api_key_policies WHERE id=?")
                    .bind(SqliteUuid(policy_id))
                    .fetch_optional(&mut *transaction)
                    .await?;
            if enabled != Some(true) {
                transaction.rollback().await?;
                return Err(RepositoryError::Validation);
            }
        }

        let user_id = Uuid::new_v4();
        let email = input.email.clone();
        let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO users \
             (id,email,display_name,role,status,balance_amount,user_group_id,default_api_key_policy_id) \
             VALUES (?,?,?,?,'invited',?,?,?) RETURNING updated_at",
        )
        .bind(SqliteUuid(user_id))
        .bind(&email)
        .bind(&input.display_name)
        .bind(input.role.as_str())
        .bind(amount_24_8(stored_balance_amount)?)
        .bind(SqliteUuid(user_group_id))
        .bind(input.default_api_key_policy_id.map(SqliteUuid))
        .fetch_one(&mut *transaction)
        .await?;
        let now = transaction_now(&mut transaction).await?;
        let expires_at = checked_expiry(now, invitation_ttl)?;
        sqlx::query(
            "INSERT INTO user_invitations (id,user_id,invited_by,token_hash,expires_at) \
             VALUES (?,?,?,?,?)",
        )
        .bind(SqliteUuid(invitation_id))
        .bind(SqliteUuid(user_id))
        .bind(SqliteUuid(actor_user_id))
        .bind(invitation_token_hash)
        .bind(SqliteTimestamp(expires_at))
        .execute(&mut *transaction)
        .await?;
        let correlation_id = Uuid::new_v4();
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: Some(actor_user_id),
                actor_type: "user",
                actor_role: Some("admin"),
                action: "invite",
                object_type: "user",
                object_id: user_id,
                before_redacted: "{}".to_owned(),
                after_redacted: to_json_text(&InvitedUserAudit {
                    id: user_id,
                    email: &email,
                    display_name: &input.display_name,
                    role: input.role.as_str(),
                    status: "invited",
                    balance_amount: input.initial_balance_amount,
                    user_group_id,
                    default_api_key_policy_id: input.default_api_key_policy_id,
                    updated_at: updated_at.0,
                })?,
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
        let mut transaction = self.write().await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        let user = sqlx::query_as::<_, UserForReinvitationRow>(
            "SELECT id,email,display_name,role,status,password_hash,balance_amount, \
                    user_group_id,default_api_key_policy_id,updated_at \
             FROM users WHERE id=? AND is_system=0",
        )
        .bind(SqliteUuid(user_id))
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

        sqlx::query(
            "UPDATE user_invitations SET revoked_at=ag_now() \
             WHERE user_id=? AND accepted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(SqliteUuid(user_id))
        .execute(&mut *transaction)
        .await?;
        let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE users SET status='invited',auth_version=auth_version+1,updated_at=ag_now() \
             WHERE id=? RETURNING updated_at",
        )
        .bind(SqliteUuid(user_id))
        .fetch_one(&mut *transaction)
        .await?;
        revoke_live_sessions(&mut transaction, user_id).await?;

        let now = transaction_now(&mut transaction).await?;
        let expires_at = checked_expiry(now, invitation_ttl)?;
        sqlx::query(
            "INSERT INTO user_invitations (id,user_id,invited_by,token_hash,expires_at) \
             VALUES (?,?,?,?,?)",
        )
        .bind(SqliteUuid(invitation_id))
        .bind(SqliteUuid(user_id))
        .bind(SqliteUuid(actor_user_id))
        .bind(invitation_token_hash)
        .bind(SqliteTimestamp(expires_at))
        .execute(&mut *transaction)
        .await?;

        let correlation_id = Uuid::new_v4();
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: Some(actor_user_id),
                actor_type: "user",
                actor_role: Some("admin"),
                action: "reinvite",
                object_type: "user",
                object_id: user_id,
                before_redacted: to_json_text(&ReinvitationBeforeAudit {
                    id: user.id.0,
                    email: user.email.as_deref(),
                    display_name: &user.display_name,
                    role: &user.role,
                    status: &user.status,
                    balance_amount: user.balance_amount.0,
                    user_group_id: user.user_group_id.0,
                    default_api_key_policy_id: user.default_api_key_policy_id.map(|value| value.0),
                    updated_at: user.updated_at.0,
                })?,
                after_redacted: to_json_text(&ReinvitationAfterAudit {
                    id: user.id.0,
                    email: user.email.as_deref(),
                    display_name: &user.display_name,
                    role: &user.role,
                    status: "invited",
                    balance_amount: user.balance_amount.0,
                    user_group_id: user.user_group_id.0,
                    default_api_key_policy_id: user.default_api_key_policy_id.map(|value| value.0),
                    invitation_id,
                    invitation_expires_at: expires_at,
                    updated_at: updated_at.0,
                })?,
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
        let mut transaction = self.write().await?;
        let now = transaction_now(&mut transaction).await?;
        let invitation = sqlx::query_as::<_, InvitationForAcceptanceRow>(
            "SELECT i.user_id,i.token_hash,i.expires_at,i.accepted_at,i.revoked_at, \
                    u.email,u.display_name,u.role,u.status \
             FROM user_invitations AS i \
             JOIN users AS u ON u.id=i.user_id \
             WHERE i.id=?",
        )
        .bind(SqliteUuid(invitation_id))
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(invitation) = invitation else {
            transaction.rollback().await?;
            return Ok(None);
        };
        if invitation.accepted_at.is_some()
            || invitation.revoked_at.is_some()
            || invitation.expires_at.0 <= now
            || invitation.status != "invited"
            || !bool::from(invitation.token_hash.ct_eq(presented_token_hash))
        {
            transaction.rollback().await?;
            return Ok(None);
        }
        let role = parse_role(&invitation.role)?;
        let auth_version = sqlx::query_scalar::<_, i64>(
            "UPDATE users \
             SET password_hash=?,password_changed_at=ag_now(),status='active', \
                 auth_version=auth_version+1,password_change_required=0, \
                 temporary_password_issued_at=NULL,temporary_password_expires_at=NULL, \
                 updated_at=ag_now() \
             WHERE id=? RETURNING auth_version",
        )
        .bind(password_hash)
        .bind(SqliteUuid(invitation.user_id.0))
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("UPDATE user_invitations SET accepted_at=ag_now() WHERE id=?")
            .bind(SqliteUuid(invitation_id))
            .execute(&mut *transaction)
            .await?;
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: Some(invitation.user_id.0),
                actor_type: "user",
                actor_role: Some(&invitation.role),
                action: "activate",
                object_type: "user",
                object_id: invitation.user_id.0,
                before_redacted: "{}".to_owned(),
                after_redacted: to_json_text(&ActivationAudit {
                    status: "active",
                    auth_version,
                })?,
                correlation_id: Uuid::new_v4(),
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(Some(SessionUser {
            id: invitation.user_id.0,
            email: invitation.email,
            display_name: invitation.display_name,
            role,
            auth_version,
            session_purpose: ConsoleSessionPurpose::Normal,
            temporary_password_expires_at: None,
        }))
    }

    pub async fn bootstrap_admin(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<Uuid, RepositoryError> {
        let mut transaction = self.write().await?;
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM users WHERE status='active' AND role='admin')",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if exists {
            transaction.rollback().await?;
            return Err(RepositoryError::Conflict);
        }
        if email.trim().is_empty() || display_name.trim().is_empty() || password_hash.is_empty() {
            transaction.rollback().await?;
            return Err(RepositoryError::Validation);
        }
        let id = Uuid::new_v4();
        let email = email.to_owned();
        sqlx::query(
            "INSERT INTO users \
             (id,email,display_name,role,status,password_hash,password_changed_at,user_group_id) \
             VALUES (?,?,?,'admin','active',?,ag_now(),?)",
        )
        .bind(SqliteUuid(id))
        .bind(&email)
        .bind(display_name)
        .bind(password_hash)
        .bind(SqliteUuid(DEFAULT_ADMIN_GROUP_ID))
        .execute(&mut *transaction)
        .await?;
        insert_audit(
            &mut transaction,
            AuditEntry {
                actor_user_id: None,
                actor_type: "system",
                actor_role: None,
                action: "bootstrap",
                object_type: "user",
                object_id: id,
                before_redacted: "{}".to_owned(),
                after_redacted: to_json_text(&BootstrapAudit {
                    id,
                    email: &email,
                    display_name,
                    role: "admin",
                    status: "active",
                })?,
                correlation_id: Uuid::new_v4(),
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(id)
    }

    async fn read(&self) -> Result<PoolConnection<Sqlite>, RepositoryError> {
        self.database.acquire_read().await.map_err(open_failure)
    }

    async fn write(&self) -> Result<Transaction<'static, Sqlite>, RepositoryError> {
        self.database.begin_write().await.map_err(open_failure)
    }
}

/// The SQLite opener reports closed/fenced/ownership failures rather than SQL errors; callers treat
/// them as internal storage failures.
fn open_failure(error: SqliteOpenError) -> RepositoryError {
    RepositoryError::from(sqlx::Error::Configuration(Box::new(error)))
}

async fn transaction_now(
    transaction: &mut Transaction<'static, Sqlite>,
) -> Result<DateTime<Utc>, RepositoryError> {
    let now = sqlx::query_scalar::<_, SqliteTimestamp>("SELECT ag_now()")
        .fetch_one(&mut **transaction)
        .await?;
    Ok(now.0)
}

fn parse_role(value: &str) -> Result<UserRole, RepositoryError> {
    UserRole::parse(value).ok_or(RepositoryError::Validation)
}

fn parse_session_purpose(value: &str) -> Result<ConsoleSessionPurpose, RepositoryError> {
    ConsoleSessionPurpose::parse(value).ok_or(RepositoryError::Validation)
}

fn normalize_numeric_24_8(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(8, RoundingStrategy::MidpointAwayFromZero)
}

fn amount_24_8(value: Decimal) -> Result<SqliteAmount, RepositoryError> {
    SqliteAmount::new(normalize_numeric_24_8(value)).map_err(|_| RepositoryError::Validation)
}

fn checked_expiry(now: DateTime<Utc>, ttl: Duration) -> Result<DateTime<Utc>, RepositoryError> {
    now.checked_add_signed(
        chrono::Duration::from_std(ttl).map_err(|_| RepositoryError::Validation)?,
    )
    .ok_or(RepositoryError::Validation)
}

fn to_json_text<T: Serialize>(value: &T) -> Result<String, RepositoryError> {
    serde_json::to_string(value).map_err(|_| RepositoryError::Validation)
}

/// PostgreSQL renders `numeric` as a JSON number when the audit object is built in SQL
/// (`user_audit`); `rust_decimal` would serialize it as a JSON string instead.
fn json_number(value: Decimal) -> Result<Box<RawValue>, RepositoryError> {
    RawValue::from_string(value.to_string()).map_err(|_| RepositoryError::Validation)
}

async fn revoke_live_sessions(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "UPDATE user_sessions SET revoked_at=ag_now() WHERE user_id=? AND revoked_at IS NULL",
    )
    .bind(SqliteUuid(user_id))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn select_profile<'e, E>(
    executor: E,
    user_id: Uuid,
) -> Result<Option<ConsoleProfile>, RepositoryError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, ProfileRow>(
        "SELECT id,email,display_name,role,status,balance_amount,created_at,updated_at \
         FROM users WHERE id=? AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(user_id))
    .fetch_optional(executor)
    .await?;
    Ok(row.map(ProfileRow::into_profile))
}

async fn select_registration_invitation_code<'e, E>(
    executor: E,
    id: Uuid,
) -> Result<Option<RegistrationInvitationCode>, RepositoryError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query_as::<_, InvitationCodeRow>(
        "SELECT id,name,max_uses,used_count,expires_at,enabled,user_group_id, \
                initial_balance_amount,created_by,last_used_at,created_at,updated_at \
         FROM registration_invitation_codes WHERE id=?",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(executor)
    .await?;
    Ok(row.map(InvitationCodeRow::into_code))
}

async fn registration_invitation_code_in_transaction(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<RegistrationInvitationCode, RepositoryError> {
    select_registration_invitation_code(&mut **transaction, id)
        .await?
        .ok_or(RepositoryError::NotFound)
}

async fn ensure_active_admin(
    transaction: &mut Transaction<'static, Sqlite>,
    user_id: Uuid,
) -> Result<(), RepositoryError> {
    let admin = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS( \
            SELECT 1 FROM users \
            WHERE id=? AND status='active' AND role='admin' AND deleted_at IS NULL \
        )",
    )
    .bind(SqliteUuid(user_id))
    .fetch_one(&mut **transaction)
    .await?;
    if admin {
        Ok(())
    } else {
        Err(RepositoryError::NotFound)
    }
}

async fn ensure_user_group(
    transaction: &mut Transaction<'static, Sqlite>,
    user_group_id: Uuid,
) -> Result<(), RepositoryError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM user_groups WHERE id=? AND deleted_at IS NULL)",
    )
    .bind(SqliteUuid(user_group_id))
    .fetch_one(&mut **transaction)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(RepositoryError::Validation)
    }
}

async fn registration_code_name_taken(
    transaction: &mut Transaction<'static, Sqlite>,
    name: &str,
    excluding: Uuid,
) -> Result<bool, RepositoryError> {
    let taken = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM registration_invitation_codes WHERE name=? AND id<>?)",
    )
    .bind(name)
    .bind(SqliteUuid(excluding))
    .fetch_one(&mut **transaction)
    .await?;
    Ok(taken)
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

/// Canonical user audit payload shared by the temporary-password and completion operations. It
/// mirrors the PostgreSQL `user_audit` projection and never contains password material.
async fn user_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    id: Uuid,
) -> Result<String, RepositoryError> {
    let row = sqlx::query_as::<_, UserAuditRow>(
        "SELECT u.id,u.email,u.display_name,u.role,u.status,u.password_hash, \
                u.password_change_required,u.temporary_password_expires_at, \
                u.user_group_id,g.system_role AS user_group_system_role, \
                u.default_api_key_policy_id, \
                COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id) \
                    AS effective_api_key_policy_id, \
                u.websocket_enabled,u.balance_amount,u.deleted_at,u.deleted_by, \
                u.created_at,u.updated_at \
         FROM users AS u \
         JOIN user_groups AS g ON g.id=u.user_group_id \
         WHERE u.id=? AND u.is_system=0",
    )
    .bind(SqliteUuid(id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    let balance_amount = json_number(row.balance_amount.0)?;
    let can_reissue_invitation = row.password_hash.is_none()
        && row.email.is_some()
        && matches!(row.status.as_str(), "invited" | "suspended" | "disabled");
    to_json_text(&UserAudit {
        id: row.id.0,
        email: row.email,
        display_name: row.display_name,
        role: row.role,
        status: row.status,
        can_reissue_invitation,
        password_change_required: row.password_change_required,
        temporary_password_expires_at: row.temporary_password_expires_at.map(|value| value.0),
        user_group_id: row.user_group_id.0,
        user_group_system_role: row.user_group_system_role,
        default_api_key_policy_id: row.default_api_key_policy_id.map(|value| value.0),
        effective_api_key_policy_id: row.effective_api_key_policy_id.map(|value| value.0),
        websocket_enabled: row.websocket_enabled,
        balance_amount,
        deleted_at: row.deleted_at.map(|value| value.0),
        deleted_by: row.deleted_by.map(|value| value.0),
        created_at: row.created_at.0,
        updated_at: row.updated_at.0,
    })
}

fn registration_invitation_code_audit(
    code: &RegistrationInvitationCode,
) -> Result<String, RepositoryError> {
    to_json_text(&RegistrationCodeAudit {
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
}

struct AuditEntry<'a> {
    actor_user_id: Option<Uuid>,
    actor_type: &'a str,
    actor_role: Option<&'a str>,
    action: &'a str,
    object_type: &'a str,
    object_id: Uuid,
    before_redacted: String,
    after_redacted: String,
    correlation_id: Uuid,
}

async fn insert_audit(
    transaction: &mut Transaction<'static, Sqlite>,
    entry: AuditEntry<'_>,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO audit_logs \
         (id,actor_user_id,actor_type,actor_role,action,object_type,object_id, \
          before_redacted,after_redacted,correlation_id) \
         VALUES (?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(SqliteUuid(Uuid::new_v4()))
    .bind(entry.actor_user_id.map(SqliteUuid))
    .bind(entry.actor_type)
    .bind(entry.actor_role)
    .bind(entry.action)
    .bind(entry.object_type)
    .bind(SqliteUuid(entry.object_id))
    .bind(entry.before_redacted)
    .bind(entry.after_redacted)
    .bind(entry.correlation_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct LoginUserRow {
    id: SqliteUuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    password_hash: Option<String>,
    auth_version: i64,
    password_change_required: bool,
    temporary_password_expires_at: Option<SqliteTimestamp>,
}

impl LoginUserRow {
    fn into_login_user(self) -> LoginUser {
        LoginUser {
            id: self.id.0,
            email: self.email,
            display_name: self.display_name,
            role: self.role,
            status: self.status,
            password_hash: self.password_hash,
            auth_version: self.auth_version,
            password_change_required: self.password_change_required,
            temporary_password_expires_at: self.temporary_password_expires_at.map(|v| v.0),
        }
    }
}

#[derive(FromRow)]
struct IdentityRow {
    user_id: SqliteUuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    auth_version: i64,
    password_change_required: bool,
    temporary_password_expires_at: Option<SqliteTimestamp>,
    session_id: SqliteUuid,
    expires_at: SqliteTimestamp,
    revoked_at: Option<SqliteTimestamp>,
    session_purpose: String,
}

#[derive(FromRow)]
struct PasswordUserRow {
    id: SqliteUuid,
    password_hash: Option<String>,
    status: String,
    role: String,
    auth_version: i64,
    password_change_required: bool,
    temporary_password_expires_at: Option<SqliteTimestamp>,
}

impl PasswordUserRow {
    fn into_password_user(self) -> PasswordUser {
        PasswordUser {
            id: self.id.0,
            password_hash: self.password_hash,
            status: self.status,
            role: self.role,
            auth_version: self.auth_version,
            password_change_required: self.password_change_required,
            temporary_password_expires_at: self.temporary_password_expires_at.map(|v| v.0),
        }
    }
}

#[derive(FromRow)]
struct SessionForRotationRow {
    user_id: SqliteUuid,
    refresh_token_hash: Vec<u8>,
    expires_at: SqliteTimestamp,
    revoked_at: Option<SqliteTimestamp>,
    purpose: String,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    auth_version: i64,
    password_change_required: bool,
    temporary_password_expires_at: Option<SqliteTimestamp>,
}

#[derive(FromRow)]
struct SessionRow {
    id: SqliteUuid,
    user_agent: Option<String>,
    created_at: SqliteTimestamp,
    last_seen_at: SqliteTimestamp,
    expires_at: SqliteTimestamp,
    revoked_at: Option<SqliteTimestamp>,
}

#[derive(FromRow)]
struct ProfileRow {
    id: SqliteUuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    balance_amount: SqliteAmount,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl ProfileRow {
    fn into_profile(self) -> ConsoleProfile {
        ConsoleProfile {
            id: self.id.0,
            email: self.email,
            display_name: self.display_name,
            role: self.role,
            status: self.status,
            balance_amount: self.balance_amount.0,
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        }
    }
}

#[derive(FromRow)]
struct InvitationCodeRow {
    id: SqliteUuid,
    name: String,
    max_uses: Option<i64>,
    used_count: i64,
    expires_at: Option<SqliteTimestamp>,
    enabled: bool,
    user_group_id: SqliteUuid,
    initial_balance_amount: SqliteAmount,
    created_by: SqliteUuid,
    last_used_at: Option<SqliteTimestamp>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

impl InvitationCodeRow {
    fn into_code(self) -> RegistrationInvitationCode {
        RegistrationInvitationCode {
            id: self.id.0,
            name: self.name,
            max_uses: self.max_uses,
            used_count: self.used_count,
            expires_at: self.expires_at.map(|v| v.0),
            enabled: self.enabled,
            user_group_id: self.user_group_id.0,
            initial_balance_amount: self.initial_balance_amount.0,
            created_by: self.created_by.0,
            last_used_at: self.last_used_at.map(|v| v.0),
            created_at: self.created_at.0,
            updated_at: self.updated_at.0,
        }
    }
}

#[derive(FromRow)]
struct InvitationCodeForUseRow {
    id: SqliteUuid,
    max_uses: Option<i64>,
    used_count: i64,
    expires_at: Option<SqliteTimestamp>,
    enabled: bool,
    user_group_id: SqliteUuid,
    initial_balance_amount: SqliteAmount,
}

#[derive(FromRow)]
struct TemporaryPasswordTargetRow {
    status: String,
    has_password: bool,
    is_system: bool,
    deleted_at: Option<SqliteTimestamp>,
}

#[derive(FromRow)]
struct TemporaryPasswordCompletionRow {
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    auth_version: i64,
    password_change_required: bool,
    temporary_password_expires_at: Option<SqliteTimestamp>,
    session_purpose: String,
    session_expires_at: SqliteTimestamp,
    session_revoked_at: Option<SqliteTimestamp>,
}

#[derive(FromRow)]
struct PasswordResetAdminRow {
    id: SqliteUuid,
    email: Option<String>,
    auth_version: i64,
}

#[derive(FromRow)]
struct InvitationForAcceptanceRow {
    user_id: SqliteUuid,
    token_hash: Vec<u8>,
    expires_at: SqliteTimestamp,
    accepted_at: Option<SqliteTimestamp>,
    revoked_at: Option<SqliteTimestamp>,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
}

#[derive(FromRow)]
struct UserForReinvitationRow {
    id: SqliteUuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    password_hash: Option<String>,
    balance_amount: SqliteAmount,
    user_group_id: SqliteUuid,
    default_api_key_policy_id: Option<SqliteUuid>,
    updated_at: SqliteTimestamp,
}

#[derive(FromRow)]
struct UserAuditRow {
    id: SqliteUuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    password_hash: Option<String>,
    password_change_required: bool,
    temporary_password_expires_at: Option<SqliteTimestamp>,
    user_group_id: SqliteUuid,
    user_group_system_role: Option<String>,
    default_api_key_policy_id: Option<SqliteUuid>,
    effective_api_key_policy_id: Option<SqliteUuid>,
    websocket_enabled: bool,
    balance_amount: SqliteAmount,
    deleted_at: Option<SqliteTimestamp>,
    deleted_by: Option<SqliteUuid>,
    created_at: SqliteTimestamp,
    updated_at: SqliteTimestamp,
}

#[derive(Serialize)]
struct UserAudit {
    id: Uuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    can_reissue_invitation: bool,
    password_change_required: bool,
    temporary_password_expires_at: Option<DateTime<Utc>>,
    user_group_id: Uuid,
    user_group_system_role: Option<String>,
    default_api_key_policy_id: Option<Uuid>,
    effective_api_key_policy_id: Option<Uuid>,
    websocket_enabled: bool,
    balance_amount: Box<RawValue>,
    deleted_at: Option<DateTime<Utc>>,
    deleted_by: Option<Uuid>,
    created_at: DateTime<Utc>,
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
    initial_balance_amount: Decimal,
    created_by: Uuid,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct RegisteredUserAudit<'a> {
    id: Uuid,
    email: &'a str,
    display_name: &'a str,
    role: &'static str,
    status: &'static str,
    balance_amount: Decimal,
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
    balance_amount: Decimal,
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
    balance_amount: Decimal,
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
    balance_amount: Decimal,
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

#[derive(Serialize)]
struct ResetAdminBeforeAudit<'a> {
    email: Option<&'a str>,
    role: &'static str,
    status: &'static str,
}

#[derive(Serialize)]
struct ResetAdminAfterAudit<'a> {
    email: Option<&'a str>,
    role: &'static str,
    status: &'static str,
    auth_version: i64,
    password_changed: bool,
}

#[derive(Serialize)]
struct BootstrapAudit<'a> {
    id: Uuid,
    email: &'a str,
    display_name: &'a str,
    role: &'static str,
    status: &'static str,
}
