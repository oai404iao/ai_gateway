//! SQLite persistence for the ported Console authentication and account lifecycle.

mod account;

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::{FromRow, SqlitePool};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::domain::{ConsoleSessionPurpose, UserRole};

use super::super::{
    auth::{
        ConsoleSession, ConsoleSessionState, LiveConsoleIdentity, LoginUser, PasswordUser,
        SessionRotation, SessionUser,
    },
    error::RepositoryError,
};

/// Feature-gated SQLite implementation of the ported Console auth/account storage.
///
/// This repository is directly constructible for backend contract tests. It
/// is not selected by process configuration while the remaining SQLite
/// repositories and runtime dispatch are incomplete.
#[derive(Clone)]
pub struct SqliteAuthRepository {
    pool: SqlitePool,
}

impl SqliteAuthRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn find_login_user(&self, email: &str) -> Result<Option<LoginUser>, RepositoryError> {
        sqlx::query_as::<_, LoginUser>(
            "SELECT id,email,display_name,role,status,password_hash,auth_version, \
                    password_change_required,temporary_password_expires_at \
             FROM users WHERE email=?1",
        )
        .bind(canonical_email(email))
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn validate_console_identity(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        auth_version: i64,
    ) -> Result<Option<LiveConsoleIdentity>, RepositoryError> {
        let identity = sqlx::query_as::<_, LiveConsoleIdentityRow>(
            "SELECT u.id AS user_id,u.email,u.display_name,u.role,u.status,u.auth_version, \
                    u.password_change_required,u.temporary_password_expires_at, \
                    s.id AS session_id,s.expires_at,s.revoked_at, \
                    s.purpose AS session_purpose \
             FROM users AS u \
             JOIN user_sessions AS s ON s.user_id=u.id \
             WHERE u.id=?1 AND s.id=?2 AND u.auth_version=?3",
        )
        .bind(user_id)
        .bind(session_id)
        .bind(auth_version)
        .fetch_optional(&self.pool)
        .await?;
        let Some(identity) = identity else {
            return Ok(None);
        };
        let now = Utc::now();
        let purpose = ConsoleSessionPurpose::parse(&identity.session_purpose);
        if identity.status != "active"
            || identity.revoked_at.is_some()
            || identity.expires_at <= now
            || match purpose {
                Some(ConsoleSessionPurpose::Normal) => identity.password_change_required,
                Some(ConsoleSessionPurpose::PasswordChange) => {
                    !identity.password_change_required
                        || identity
                            .temporary_password_expires_at
                            .is_none_or(|expiry| expiry <= now)
                }
                None => true,
            }
        {
            return Ok(None);
        }
        Ok(Some(identity.into_identity()))
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
        sqlx::query(
            "INSERT INTO user_sessions \
             (id,user_id,refresh_token_hash,expires_at,user_agent,purpose) \
             VALUES (?1,?2,?3,?4,?5,?6)",
        )
        .bind(id)
        .bind(user_id)
        .bind(refresh_token_hash)
        .bind(timestamp_text(expires_at))
        .bind(user_agent)
        .bind(purpose.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn password_user(
        &self,
        user_id: Uuid,
    ) -> Result<Option<PasswordUser>, RepositoryError> {
        sqlx::query_as::<_, PasswordUser>(
            "SELECT id,password_hash,status,role,auth_version,password_change_required, \
                    temporary_password_expires_at \
             FROM users WHERE id=?1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    /// Rotates a refresh credential under an immediate SQLite write lock.
    ///
    /// A mismatched credential for an otherwise active session is treated as
    /// replay and revokes that session, matching the PostgreSQL repository.
    pub async fn rotate_session(
        &self,
        session_id: Uuid,
        presented_hash: &[u8],
        next_hash: &[u8],
        next_expires_at: DateTime<Utc>,
        user_agent: Option<&str>,
    ) -> Result<SessionRotation, RepositoryError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = Utc::now();
        let session = sqlx::query_as::<_, SessionForRotation>(
            "SELECT s.user_id,s.refresh_token_hash,s.expires_at,s.revoked_at,s.purpose, \
                    u.email,u.display_name,u.role,u.status,u.auth_version, \
                    u.password_change_required,u.temporary_password_expires_at \
             FROM user_sessions AS s \
             JOIN users AS u ON u.id=s.user_id \
             WHERE s.id=?1",
        )
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(session) = session else {
            transaction.rollback().await?;
            return Ok(SessionRotation::Invalid);
        };
        let purpose = parse_session_purpose(&session.purpose)?;
        if session.revoked_at.is_some()
            || session.expires_at <= now
            || session.status != "active"
            || match purpose {
                ConsoleSessionPurpose::Normal => session.password_change_required,
                ConsoleSessionPurpose::PasswordChange => {
                    !session.password_change_required
                        || session
                            .temporary_password_expires_at
                            .is_none_or(|expiry| expiry <= now)
                }
            }
        {
            transaction.rollback().await?;
            return Ok(SessionRotation::Invalid);
        }
        let now_text = timestamp_text(now);
        if !bool::from(session.refresh_token_hash.ct_eq(presented_hash)) {
            sqlx::query(
                "UPDATE user_sessions SET revoked_at=?2 WHERE id=?1 AND revoked_at IS NULL",
            )
            .bind(session_id)
            .bind(&now_text)
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
                    .ok_or(RepositoryError::Validation)?,
            ),
        };
        sqlx::query(
            "UPDATE user_sessions \
             SET refresh_token_hash=?2,expires_at=?3,rotated_at=?4,last_seen_at=?4, \
                 user_agent=COALESCE(?5,user_agent) \
             WHERE id=?1",
        )
        .bind(session_id)
        .bind(next_hash)
        .bind(timestamp_text(next_expires_at))
        .bind(&now_text)
        .bind(user_agent)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(SessionRotation::Rotated {
            user: SessionUser {
                id: session.user_id,
                email: session.email,
                display_name: session.display_name,
                role: parse_role(&session.role)?,
                auth_version: session.auth_version,
                session_purpose: purpose,
                temporary_password_expires_at: session.temporary_password_expires_at,
            },
            refresh_expires_at: next_expires_at,
        })
    }

    pub async fn revoke_session_for_user(
        &self,
        user_id: Uuid,
        session_id: Uuid,
    ) -> Result<bool, RepositoryError> {
        let result = sqlx::query(
            "UPDATE user_sessions SET revoked_at=?3 \
             WHERE id=?1 AND user_id=?2 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(timestamp_text(Utc::now()))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn revoke_all_sessions(&self, user_id: Uuid) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE user_sessions SET revoked_at=?2 \
             WHERE user_id=?1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(timestamp_text(Utc::now()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn revoke_other_sessions(
        &self,
        user_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<u64, RepositoryError> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE user_sessions SET revoked_at=?3 \
             WHERE user_id=?1 AND id<>?2 AND revoked_at IS NULL \
               AND julianday(expires_at)>julianday(?3)",
        )
        .bind(user_id)
        .bind(current_session_id)
        .bind(timestamp_text(now))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn sessions_for_user(
        &self,
        user_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<Vec<ConsoleSession>, RepositoryError> {
        let now = Utc::now();
        let sessions = sqlx::query_as::<_, StoredConsoleSession>(
            "SELECT id,user_agent,created_at,last_seen_at,expires_at,revoked_at \
             FROM user_sessions WHERE user_id=?1 \
             ORDER BY (id=?2) DESC, \
                      (revoked_at IS NULL AND julianday(expires_at)>julianday(?3)) DESC, \
                      julianday(created_at) DESC,id",
        )
        .bind(user_id)
        .bind(current_session_id)
        .bind(timestamp_text(now))
        .fetch_all(&self.pool)
        .await?;
        Ok(sessions
            .into_iter()
            .map(|session| ConsoleSession {
                id: session.id,
                user_agent: session.user_agent,
                created_at: session.created_at,
                last_seen_at: session.last_seen_at,
                expires_at: session.expires_at,
                revoked_at: session.revoked_at,
                state: if session.revoked_at.is_some() {
                    ConsoleSessionState::Revoked
                } else if session.expires_at <= now {
                    ConsoleSessionState::Expired
                } else {
                    ConsoleSessionState::Active
                },
                is_current: session.id == current_session_id,
            })
            .collect())
    }
}

fn timestamp_text(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// SQLite's built-in `lower()` is ASCII-only, so every future SQLite user
/// writer must persist this canonical form and lookups must compare it
/// directly. Runtime SQLite dispatch remains disabled until those writers
/// exist.
pub(super) fn canonical_email(value: &str) -> String {
    value.trim().to_lowercase()
}

fn parse_role(value: &str) -> Result<UserRole, RepositoryError> {
    UserRole::parse(value).ok_or(RepositoryError::Validation)
}

fn parse_session_purpose(value: &str) -> Result<ConsoleSessionPurpose, RepositoryError> {
    ConsoleSessionPurpose::parse(value).ok_or(RepositoryError::Validation)
}

#[derive(FromRow)]
struct LiveConsoleIdentityRow {
    user_id: Uuid,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    auth_version: i64,
    password_change_required: bool,
    temporary_password_expires_at: Option<DateTime<Utc>>,
    session_id: Uuid,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    session_purpose: String,
}

impl LiveConsoleIdentityRow {
    fn into_identity(self) -> LiveConsoleIdentity {
        LiveConsoleIdentity {
            user_id: self.user_id,
            email: self.email,
            display_name: self.display_name,
            role: self.role,
            status: self.status,
            auth_version: self.auth_version,
            session_id: self.session_id,
            expires_at: self.expires_at,
            revoked_at: self.revoked_at,
            session_purpose: self.session_purpose,
        }
    }
}

#[derive(FromRow)]
struct SessionForRotation {
    user_id: Uuid,
    refresh_token_hash: Vec<u8>,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    purpose: String,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    auth_version: i64,
    password_change_required: bool,
    temporary_password_expires_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct StoredConsoleSession {
    id: Uuid,
    user_agent: Option<String>,
    created_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}
