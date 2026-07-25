//! PostgreSQL persistence for Console identities, invitations, and sessions.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::domain::UserRole;

use super::RepositoryError;

#[derive(Clone)]
pub struct AuthRepository {
    pool: PgPool,
}

impl AuthRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn find_login_user(&self, email: &str) -> Result<Option<LoginUser>, RepositoryError> {
        sqlx::query_as::<_, LoginUser>(
            "SELECT id,email,display_name,role,status,password_hash,auth_version,default_api_key_policy_id \
             FROM users WHERE lower(email) = lower($1)",
        )
        .bind(email)
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
        sqlx::query_as::<_, LiveConsoleIdentity>(
            "SELECT u.id AS user_id,u.email,u.display_name,u.role,u.status,u.auth_version, \
                    s.id AS session_id,s.expires_at,s.revoked_at \
             FROM users AS u \
             JOIN user_sessions AS s ON s.user_id=u.id \
             WHERE u.id=$1 AND s.id=$2 AND u.auth_version=$3 \
               AND u.status='active' AND s.revoked_at IS NULL AND s.expires_at > now()",
        )
        .bind(user_id)
        .bind(session_id)
        .bind(auth_version)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn create_session(
        &self,
        id: Uuid,
        user_id: Uuid,
        refresh_token_hash: &[u8],
        expires_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO user_sessions (id,user_id,refresh_token_hash,expires_at) \
             VALUES ($1,$2,$3,$4)",
        )
        .bind(id)
        .bind(user_id)
        .bind(refresh_token_hash)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn password_user(
        &self,
        user_id: Uuid,
    ) -> Result<Option<PasswordUser>, RepositoryError> {
        sqlx::query_as::<_, PasswordUser>("SELECT id,password_hash,status FROM users WHERE id=$1")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::from)
    }

    /// Rotates a refresh credential. A mismatched credential for an otherwise
    /// active session is treated as replay and revokes that session.
    pub async fn rotate_session(
        &self,
        session_id: Uuid,
        presented_hash: &[u8],
        next_hash: &[u8],
        next_expires_at: DateTime<Utc>,
    ) -> Result<SessionRotation, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let session = sqlx::query_as::<_, SessionForRotation>(
            "SELECT s.user_id,s.refresh_token_hash,s.expires_at,s.revoked_at, \
                    u.email,u.display_name,u.role,u.status,u.auth_version \
             FROM user_sessions AS s \
             JOIN users AS u ON u.id=s.user_id \
             WHERE s.id=$1 FOR UPDATE",
        )
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(session) = session else {
            transaction.rollback().await?;
            return Ok(SessionRotation::Invalid);
        };
        if session.revoked_at.is_some()
            || session.expires_at <= Utc::now()
            || session.status != "active"
        {
            transaction.rollback().await?;
            return Ok(SessionRotation::Invalid);
        }
        if !bool::from(session.refresh_token_hash.ct_eq(presented_hash)) {
            sqlx::query(
                "UPDATE user_sessions SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
            )
            .bind(session_id)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Ok(SessionRotation::Replayed);
        }
        sqlx::query(
            "UPDATE user_sessions \
             SET refresh_token_hash=$2,expires_at=$3,rotated_at=now(),last_seen_at=now() \
             WHERE id=$1",
        )
        .bind(session_id)
        .bind(next_hash)
        .bind(next_expires_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(SessionRotation::Rotated(SessionUser {
            id: session.user_id,
            email: session.email,
            display_name: session.display_name,
            role: parse_role(&session.role)?,
            auth_version: session.auth_version,
        }))
    }

    pub async fn revoke_session_for_user(
        &self,
        user_id: Uuid,
        session_id: Uuid,
    ) -> Result<bool, RepositoryError> {
        let result = sqlx::query(
            "UPDATE user_sessions SET revoked_at=now() \
             WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn revoke_all_sessions(&self, user_id: Uuid) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE user_sessions SET revoked_at=now() \
             WHERE user_id=$1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn sessions_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<ConsoleSession>, RepositoryError> {
        sqlx::query_as::<_, ConsoleSession>(
            "SELECT id,created_at,last_seen_at,expires_at,revoked_at \
             FROM user_sessions WHERE user_id=$1 ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn profile(&self, user_id: Uuid) -> Result<Option<ConsoleProfile>, RepositoryError> {
        sqlx::query_as::<_, ConsoleProfile>(
            "SELECT id,email,display_name,role,status,balance_amount,created_at,updated_at \
             FROM users WHERE id=$1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn update_display_name(
        &self,
        user_id: Uuid,
        display_name: &str,
    ) -> Result<Option<ConsoleProfile>, RepositoryError> {
        sqlx::query_as::<_, ConsoleProfile>(
            "UPDATE users SET display_name=$2 WHERE id=$1 AND status='active' \
             RETURNING id,email,display_name,role,status,balance_amount,created_at,updated_at",
        )
        .bind(user_id)
        .bind(display_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn change_password(
        &self,
        user_id: Uuid,
        password_hash: &str,
    ) -> Result<bool, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let changed = sqlx::query(
            "UPDATE users \
             SET password_hash=$2,password_changed_at=now(),auth_version=auth_version+1 \
             WHERE id=$1 AND status='active'",
        )
        .bind(user_id)
        .bind(password_hash)
        .execute(&mut *transaction)
        .await?;
        if changed.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE user_sessions SET revoked_at=now() WHERE user_id=$1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Emergency operator recovery for an active Console administrator.
    ///
    /// The caller supplies an already validated Argon2 password hash. As with
    /// a self-service password change, every existing session is revoked.
    pub async fn reset_active_admin_password(
        &self,
        email: &str,
        password_hash: &str,
    ) -> Result<bool, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let admin = sqlx::query_as::<_, PasswordResetAdmin>(
            "UPDATE users \
             SET password_hash=$2,password_changed_at=now(),auth_version=auth_version+1 \
             WHERE lower(email)=lower($1) AND role='admin' AND status='active' \
             RETURNING id,email,auth_version",
        )
        .bind(email)
        .bind(password_hash)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(admin) = admin else {
            transaction.rollback().await?;
            return Ok(false);
        };
        sqlx::query(
            "UPDATE user_sessions SET revoked_at=now() WHERE user_id=$1 AND revoked_at IS NULL",
        )
        .bind(admin.id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO audit_logs \
             (id,actor_type,action,object_type,object_id,before_redacted,after_redacted,correlation_id) \
             VALUES ($1,'system','reset_password','user',$2,$3,$4,$5)",
        )
        .bind(Uuid::new_v4())
        .bind(admin.id)
        .bind(json!({
            "email": admin.email.clone(),
            "role": "admin",
            "status": "active",
        }))
        .bind(json!({
            "email": admin.email,
            "role": "admin",
            "status": "active",
            "auth_version": admin.auth_version,
            "password_changed": true,
        }))
        .bind(Uuid::new_v4().to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn invite_user(
        &self,
        actor_user_id: Uuid,
        input: InviteUserInput,
        invitation_id: Uuid,
        invitation_token_hash: &[u8],
        invitation_ttl: Duration,
    ) -> Result<InvitationCreated, RepositoryError> {
        let mut transaction = begin_serializable(&self.pool).await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        if input.email.trim().is_empty() || input.display_name.trim().is_empty() {
            transaction.rollback().await?;
            return Err(RepositoryError::Validation);
        }
        if input.initial_balance_amount.is_sign_negative() {
            transaction.rollback().await?;
            return Err(RepositoryError::Validation);
        }
        if let Some(policy_id) = input.default_api_key_policy_id {
            let enabled = sqlx::query_scalar::<_, bool>(
                "SELECT enabled FROM api_key_policies WHERE id=$1 FOR KEY SHARE",
            )
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
        let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
            "INSERT INTO users \
             (id,email,display_name,role,status,balance_amount,default_api_key_policy_id) \
             VALUES ($1,$2,$3,$4,'invited',$5,$6) RETURNING updated_at",
        )
        .bind(user_id)
        .bind(&input.email)
        .bind(&input.display_name)
        .bind(input.role.as_str())
        .bind(input.initial_balance_amount)
        .bind(input.default_api_key_policy_id)
        .fetch_one(&mut *transaction)
        .await?;
        let expires_at = Utc::now()
            + chrono::Duration::from_std(invitation_ttl)
                .map_err(|_| RepositoryError::Validation)?;
        sqlx::query(
            "INSERT INTO user_invitations (id,user_id,invited_by,token_hash,expires_at) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(invitation_id)
        .bind(user_id)
        .bind(actor_user_id)
        .bind(invitation_token_hash)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;
        let correlation_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO audit_logs \
             (id,actor_user_id,actor_type,actor_role,action,object_type,object_id,before_redacted,after_redacted,correlation_id) \
             VALUES ($1,$2,'user','admin','invite','user',$3,'{}',$4,$5)",
        )
        .bind(Uuid::new_v4())
        .bind(actor_user_id)
        .bind(user_id)
        .bind(json!({
            "id": user_id,
            "email": input.email,
            "display_name": input.display_name,
            "role": input.role.as_str(),
            "status": "invited",
            "balance_amount": input.initial_balance_amount,
            "default_api_key_policy_id": input.default_api_key_policy_id,
            "updated_at": updated_at,
        }))
        .bind(correlation_id.to_string())
        .execute(&mut *transaction)
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
        let mut transaction = begin_serializable(&self.pool).await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        let user = sqlx::query_as::<_, UserForReinvitation>(
            "SELECT id,email,display_name,role,status,password_hash,balance_amount, \
                    default_api_key_policy_id,updated_at \
             FROM users WHERE id=$1 AND NOT is_system FOR UPDATE",
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

        sqlx::query(
            "UPDATE user_invitations SET revoked_at=now() \
             WHERE user_id=$1 AND accepted_at IS NULL AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
            "UPDATE users SET status='invited',auth_version=auth_version+1 \
             WHERE id=$1 RETURNING updated_at",
        )
        .bind(user_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE user_sessions SET revoked_at=now() \
             WHERE user_id=$1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;

        let expires_at = Utc::now()
            + chrono::Duration::from_std(invitation_ttl)
                .map_err(|_| RepositoryError::Validation)?;
        sqlx::query(
            "INSERT INTO user_invitations \
             (id,user_id,invited_by,token_hash,expires_at) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(invitation_id)
        .bind(user_id)
        .bind(actor_user_id)
        .bind(invitation_token_hash)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;

        let correlation_id = Uuid::new_v4();
        let before = json!({
            "id": user.id,
            "email": user.email,
            "display_name": user.display_name,
            "role": user.role,
            "status": user.status,
            "balance_amount": user.balance_amount,
            "default_api_key_policy_id": user.default_api_key_policy_id,
            "updated_at": user.updated_at,
        });
        let after = json!({
            "id": user_id,
            "email": before["email"],
            "display_name": before["display_name"],
            "role": before["role"],
            "status": "invited",
            "balance_amount": before["balance_amount"],
            "default_api_key_policy_id": before["default_api_key_policy_id"],
            "invitation_id": invitation_id,
            "invitation_expires_at": expires_at,
            "updated_at": updated_at,
        });
        sqlx::query(
            "INSERT INTO audit_logs \
             (id,actor_user_id,actor_type,actor_role,action,object_type,object_id,before_redacted,after_redacted,correlation_id) \
             VALUES ($1,$2,'user','admin','reinvite','user',$3,$4,$5,$6)",
        )
        .bind(Uuid::new_v4())
        .bind(actor_user_id)
        .bind(user_id)
        .bind(before)
        .bind(after)
        .bind(correlation_id.to_string())
        .execute(&mut *transaction)
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
        let mut transaction = begin_serializable(&self.pool).await?;
        let invitation = sqlx::query_as::<_, InvitationForAcceptance>(
            "SELECT i.user_id,i.token_hash,i.expires_at,i.accepted_at,i.revoked_at, \
                    u.email,u.display_name,u.role,u.status \
             FROM user_invitations AS i \
             JOIN users AS u ON u.id=i.user_id \
             WHERE i.id=$1 FOR UPDATE",
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
            || invitation.expires_at <= Utc::now()
            || invitation.status != "invited"
            || !bool::from(invitation.token_hash.ct_eq(presented_token_hash))
        {
            transaction.rollback().await?;
            return Ok(None);
        }
        let auth_version = sqlx::query_scalar::<_, i64>(
            "UPDATE users \
             SET password_hash=$2,password_changed_at=now(),status='active',auth_version=auth_version+1 \
             WHERE id=$1 RETURNING auth_version",
        )
        .bind(invitation.user_id)
        .bind(password_hash)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("UPDATE user_invitations SET accepted_at=now() WHERE id=$1")
            .bind(invitation_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO audit_logs \
             (id,actor_user_id,actor_type,actor_role,action,object_type,object_id,before_redacted,after_redacted,correlation_id) \
             VALUES ($1,$2,'user',$3,'activate','user',$2,'{}',$4,$5)",
        )
        .bind(Uuid::new_v4())
        .bind(invitation.user_id)
        .bind(&invitation.role)
        .bind(json!({"status":"active", "auth_version": auth_version}))
        .bind(Uuid::new_v4().to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(Some(SessionUser {
            id: invitation.user_id,
            email: invitation.email,
            display_name: invitation.display_name,
            role: parse_role(&invitation.role)?,
            auth_version,
        }))
    }

    pub async fn bootstrap_admin(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<Uuid, RepositoryError> {
        let mut transaction = begin_serializable(&self.pool).await?;
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
        sqlx::query(
            "INSERT INTO users (id,email,display_name,role,status,password_hash,password_changed_at) \
             VALUES ($1,$2,$3,'admin','active',$4,now())",
        )
        .bind(id)
        .bind(email)
        .bind(display_name)
        .bind(password_hash)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO audit_logs \
             (id,actor_type,action,object_type,object_id,before_redacted,after_redacted,correlation_id) \
             VALUES ($1,'system','bootstrap','user',$2,'{}',$3,$4)",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(json!({"id":id,"email":email,"display_name":display_name,"role":"admin","status":"active"}))
        .bind(Uuid::new_v4().to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(id)
    }
}

async fn begin_serializable(pool: &PgPool) -> Result<Transaction<'_, Postgres>, RepositoryError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await?;
    Ok(transaction)
}

async fn ensure_active_admin(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), RepositoryError> {
    let admin = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND status='active' AND role='admin')",
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

fn parse_role(value: &str) -> Result<UserRole, RepositoryError> {
    UserRole::parse(value).ok_or(RepositoryError::Validation)
}

#[derive(Clone, FromRow)]
pub struct LoginUser {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub password_hash: Option<String>,
    pub auth_version: i64,
    pub default_api_key_policy_id: Option<Uuid>,
}

#[derive(Clone, FromRow)]
pub struct LiveConsoleIdentity {
    pub user_id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub auth_version: i64,
    pub session_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
pub struct PasswordUser {
    pub id: Uuid,
    pub password_hash: Option<String>,
    pub status: String,
}

#[derive(FromRow)]
struct PasswordResetAdmin {
    id: Uuid,
    email: Option<String>,
    auth_version: i64,
}

#[derive(Clone)]
pub struct SessionUser {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: UserRole,
    pub auth_version: i64,
}

#[derive(FromRow)]
struct SessionForRotation {
    user_id: Uuid,
    refresh_token_hash: Vec<u8>,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    email: Option<String>,
    display_name: String,
    role: String,
    status: String,
    auth_version: i64,
}

pub enum SessionRotation {
    Rotated(SessionUser),
    Invalid,
    Replayed,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct ConsoleSession {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct ConsoleProfile {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub balance_amount: rust_decimal::Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct InviteUserInput {
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub initial_balance_amount: rust_decimal::Decimal,
    pub default_api_key_policy_id: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub struct InvitationCreated {
    pub user_id: Uuid,
    pub invitation_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub correlation_id: Uuid,
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
    balance_amount: rust_decimal::Decimal,
    default_api_key_policy_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
}
