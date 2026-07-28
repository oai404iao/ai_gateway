//! PostgreSQL persistence for Console identities, invitations, registration
//! codes, and sessions.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::domain::UserRole;

use super::{DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID, RepositoryError};

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
            "SELECT id,email,display_name,role,status,password_hash,auth_version \
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
        user_agent: Option<&str>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO user_sessions \
             (id,user_id,refresh_token_hash,expires_at,user_agent) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(id)
        .bind(user_id)
        .bind(refresh_token_hash)
        .bind(expires_at)
        .bind(user_agent)
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
        user_agent: Option<&str>,
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
             SET refresh_token_hash=$2,expires_at=$3,rotated_at=now(),last_seen_at=now(), \
                 user_agent=COALESCE($4,user_agent) \
             WHERE id=$1",
        )
        .bind(session_id)
        .bind(next_hash)
        .bind(next_expires_at)
        .bind(user_agent)
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

    pub async fn revoke_other_sessions(
        &self,
        user_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<u64, RepositoryError> {
        let result = sqlx::query(
            "UPDATE user_sessions SET revoked_at=now() \
             WHERE user_id=$1 AND id<>$2 AND revoked_at IS NULL AND expires_at>now()",
        )
        .bind(user_id)
        .bind(current_session_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn sessions_for_user(
        &self,
        user_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<Vec<ConsoleSession>, RepositoryError> {
        let sessions = sqlx::query_as::<_, StoredConsoleSession>(
            "SELECT id,user_agent,created_at,last_seen_at,expires_at,revoked_at \
             FROM user_sessions WHERE user_id=$1 \
             ORDER BY (id=$2) DESC, \
                      (revoked_at IS NULL AND expires_at>now()) DESC, \
                      created_at DESC",
        )
        .bind(user_id)
        .bind(current_session_id)
        .fetch_all(&self.pool)
        .await?;
        let now = Utc::now();
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
        sqlx::query_as::<_, RegistrationInvitationCode>(
            "SELECT id,name,max_uses,used_count,expires_at,enabled,user_group_id, \
                    initial_balance_amount,created_by,last_used_at,created_at,updated_at \
             FROM registration_invitation_codes WHERE id=$1",
        )
        .bind(id)
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

    pub async fn create_registration_invitation_code(
        &self,
        actor_user_id: Uuid,
        code_hash: &[u8],
        input: RegistrationInvitationCodeInput,
    ) -> Result<RegistrationInvitationCodeMutation, RepositoryError> {
        let mut transaction = begin_serializable(&self.pool).await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        validate_registration_invitation_code_input(&input, 0)?;
        ensure_registration_user_group(&mut transaction, input.user_group_id).await?;

        let id = Uuid::new_v4();
        let inserted = sqlx::query_scalar::<_, DateTime<Utc>>(
            "INSERT INTO registration_invitation_codes \
             (id,name,code_hash,max_uses,expires_at,enabled,user_group_id, \
              initial_balance_amount,created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
             ON CONFLICT DO NOTHING RETURNING updated_at",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(code_hash)
        .bind(input.max_uses)
        .bind(input.expires_at)
        .bind(input.enabled)
        .bind(input.user_group_id)
        .bind(input.initial_balance_amount)
        .bind(actor_user_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if inserted.is_none() {
            transaction.rollback().await?;
            return Err(RepositoryError::RegistrationInvitationCodeConflict);
        }

        let after = registration_invitation_code_for_update(&mut transaction, id).await?;
        let correlation_id = Uuid::new_v4();
        insert_registration_invitation_code_audit(
            &mut transaction,
            actor_user_id,
            "create",
            id,
            json!({}),
            registration_invitation_code_audit(&after),
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
        let mut transaction = begin_serializable(&self.pool).await?;
        ensure_active_admin(&mut transaction, actor_user_id).await?;
        let before = registration_invitation_code_for_update(&mut transaction, id).await?;
        if before.updated_at != expected_updated_at {
            transaction.rollback().await?;
            return Err(RepositoryError::Conflict);
        }
        validate_registration_invitation_code_input(&input, before.used_count)?;
        ensure_registration_user_group(&mut transaction, input.user_group_id).await?;

        let updated = sqlx::query_scalar::<_, DateTime<Utc>>(
            "UPDATE registration_invitation_codes SET \
             name=$2,max_uses=$3,expires_at=$4,enabled=$5,user_group_id=$6, \
             initial_balance_amount=$7 \
             WHERE id=$1 AND updated_at=$8 RETURNING updated_at",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(input.max_uses)
        .bind(input.expires_at)
        .bind(input.enabled)
        .bind(input.user_group_id)
        .bind(input.initial_balance_amount)
        .bind(expected_updated_at)
        .fetch_optional(&mut *transaction)
        .await;
        let updated = match updated {
            Ok(updated) => updated,
            Err(error) if unique_violation(&error) => {
                transaction.rollback().await?;
                return Err(RepositoryError::RegistrationInvitationCodeConflict);
            }
            Err(error) => return Err(error.into()),
        };
        if updated.is_none() {
            transaction.rollback().await?;
            return Err(RepositoryError::Conflict);
        }

        let after = registration_invitation_code_for_update(&mut transaction, id).await?;
        let correlation_id = Uuid::new_v4();
        insert_registration_invitation_code_audit(
            &mut transaction,
            actor_user_id,
            "update",
            id,
            registration_invitation_code_audit(&before),
            registration_invitation_code_audit(&after),
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
        let mut transaction = begin_serializable(&self.pool).await?;
        let invitation = sqlx::query_as::<_, RegistrationInvitationCodeForUse>(
            "SELECT id,max_uses,used_count,expires_at,enabled,user_group_id, \
                    initial_balance_amount \
             FROM registration_invitation_codes WHERE code_hash=$1 FOR UPDATE",
        )
        .bind(code_hash)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(invitation) = invitation else {
            transaction.rollback().await?;
            return Ok(RegistrationAttempt::InvalidCode);
        };
        if !invitation.enabled
            || invitation
                .expires_at
                .is_some_and(|expiry| expiry <= Utc::now())
            || invitation
                .max_uses
                .is_some_and(|maximum| invitation.used_count >= maximum)
        {
            transaction.rollback().await?;
            return Ok(RegistrationAttempt::InvalidCode);
        }

        let user_id = Uuid::new_v4();
        let auth_version = sqlx::query_scalar::<_, i64>(
            "INSERT INTO users \
             (id,email,display_name,role,status,password_hash,password_changed_at, \
              balance_amount,user_group_id) \
             VALUES ($1,$2,$3,'user','active',$4,now(),$5,$6) \
             ON CONFLICT DO NOTHING RETURNING auth_version",
        )
        .bind(user_id)
        .bind(email)
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
             SET used_count=used_count+1,last_used_at=now() WHERE id=$1",
        )
        .bind(invitation.id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO audit_logs \
             (id,actor_user_id,actor_type,actor_role,action,object_type,object_id, \
              before_redacted,after_redacted,correlation_id) \
             VALUES ($1,$2,'user','user','register','user',$2,'{}',$3,$4)",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(json!({
            "id": user_id,
            "email": email,
            "display_name": display_name,
            "role": "user",
            "status": "active",
            "balance_amount": invitation.initial_balance_amount,
            "user_group_id": invitation.user_group_id,
            "registration_invitation_code_id": invitation.id,
        }))
        .bind(Uuid::new_v4().to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(RegistrationAttempt::Registered(SessionUser {
            id: user_id,
            email: Some(email.to_owned()),
            display_name: display_name.to_owned(),
            role: UserRole::User,
            auth_version,
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
        let user_group_id = input.user_group_id.unwrap_or(match input.role {
            UserRole::User => DEFAULT_USER_GROUP_ID,
            UserRole::Admin => DEFAULT_ADMIN_GROUP_ID,
        });
        let group_exists =
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM user_groups WHERE id=$1)")
                .bind(user_group_id)
                .fetch_one(&mut *transaction)
                .await?;
        if !group_exists {
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
             (id,email,display_name,role,status,balance_amount,user_group_id,default_api_key_policy_id) \
             VALUES ($1,$2,$3,$4,'invited',$5,$6,$7) RETURNING updated_at",
        )
        .bind(user_id)
        .bind(&input.email)
        .bind(&input.display_name)
        .bind(input.role.as_str())
        .bind(input.initial_balance_amount)
        .bind(user_group_id)
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
            "user_group_id": user_group_id,
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
                    user_group_id,default_api_key_policy_id,updated_at \
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
            "user_group_id": user.user_group_id,
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
            "user_group_id": before["user_group_id"],
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
            "INSERT INTO users \
             (id,email,display_name,role,status,password_hash,password_changed_at,user_group_id) \
             VALUES ($1,$2,$3,'admin','active',$4,now(),$5)",
        )
        .bind(id)
        .bind(email)
        .bind(display_name)
        .bind(password_hash)
        .bind(DEFAULT_ADMIN_GROUP_ID)
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

async fn ensure_registration_user_group(
    transaction: &mut Transaction<'_, Postgres>,
    user_group_id: Uuid,
) -> Result<(), RepositoryError> {
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM user_groups WHERE id=$1)")
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

async fn registration_invitation_code_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<RegistrationInvitationCode, RepositoryError> {
    sqlx::query_as::<_, RegistrationInvitationCode>(
        "SELECT id,name,max_uses,used_count,expires_at,enabled,user_group_id, \
                initial_balance_amount,created_by,last_used_at,created_at,updated_at \
         FROM registration_invitation_codes WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)
}

fn registration_invitation_code_audit(code: &RegistrationInvitationCode) -> serde_json::Value {
    json!({
        "id": code.id,
        "name": code.name,
        "max_uses": code.max_uses,
        "used_count": code.used_count,
        "expires_at": code.expires_at,
        "enabled": code.enabled,
        "user_group_id": code.user_group_id,
        "initial_balance_amount": code.initial_balance_amount,
        "created_by": code.created_by,
        "last_used_at": code.last_used_at,
        "created_at": code.created_at,
        "updated_at": code.updated_at,
    })
}

async fn insert_registration_invitation_code_audit(
    transaction: &mut Transaction<'_, Postgres>,
    actor_user_id: Uuid,
    action: &str,
    id: Uuid,
    before: serde_json::Value,
    after: serde_json::Value,
    correlation_id: Uuid,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO audit_logs \
         (id,actor_user_id,actor_type,actor_role,action,object_type,object_id, \
          before_redacted,after_redacted,correlation_id) \
         VALUES ($1,$2,'user','admin',$3,'registration_invitation_code',$4,$5,$6,$7)",
    )
    .bind(Uuid::new_v4())
    .bind(actor_user_id)
    .bind(action)
    .bind(id)
    .bind(before)
    .bind(after)
    .bind(correlation_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23505")
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleSessionState {
    Active,
    Expired,
    Revoked,
}

#[derive(Clone, Debug, FromRow)]
struct StoredConsoleSession {
    id: Uuid,
    user_agent: Option<String>,
    created_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsoleSession {
    pub id: Uuid,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub state: ConsoleSessionState,
    pub is_current: bool,
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
    pub user_group_id: Option<Uuid>,
    pub default_api_key_policy_id: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub struct InvitationCreated {
    pub user_id: Uuid,
    pub invitation_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct RegistrationInvitationCode {
    pub id: Uuid,
    pub name: String,
    pub max_uses: Option<i64>,
    pub used_count: i64,
    pub expires_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub user_group_id: Uuid,
    pub initial_balance_amount: rust_decimal::Decimal,
    pub created_by: Uuid,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct RegistrationInvitationCodeInput {
    pub name: String,
    pub max_uses: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub user_group_id: Uuid,
    pub initial_balance_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug)]
pub struct RegistrationInvitationCodeMutation {
    pub id: Uuid,
    pub correlation_id: Uuid,
}

pub enum RegistrationAttempt {
    Registered(SessionUser),
    InvalidCode,
    EmailConflict,
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
    user_group_id: Uuid,
    default_api_key_policy_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct RegistrationInvitationCodeForUse {
    id: Uuid,
    max_uses: Option<i64>,
    used_count: i64,
    expires_at: Option<DateTime<Utc>>,
    enabled: bool,
    user_group_id: Uuid,
    initial_balance_amount: rust_decimal::Decimal,
}
