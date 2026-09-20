//! Explicit backend dispatch for Console identities, sessions, invitations and registration.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::ConsoleSessionPurpose;

use super::{
    ConsoleProfile, ConsoleSession, InvitationCreated, InviteUserInput, LiveConsoleIdentity,
    LoginUser, PasswordUser, PostgresAuthRepository, RegistrationAttempt,
    RegistrationInvitationCode, RegistrationInvitationCodeInput,
    RegistrationInvitationCodeMutation, RepositoryError, SessionRotation, SessionUser,
    TemporaryPasswordCreated,
};

#[cfg(feature = "sqlite-backend")]
use std::sync::Arc;

#[cfg(feature = "sqlite-backend")]
use super::sqlite::{SqliteAuthRepository, SqliteDatabase};

enum Backend {
    Postgres(PostgresAuthRepository),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(SqliteAuthRepository),
}

impl Clone for Backend {
    fn clone(&self) -> Self {
        match self {
            Self::Postgres(repository) => Self::Postgres(repository.clone()),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(repository) => Self::Sqlite(repository.clone()),
        }
    }
}

/// Console identity, invitation, registration-code, and session operations.
#[derive(Clone)]
pub struct AuthRepository {
    backend: Backend,
}

impl AuthRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            backend: Backend::Postgres(PostgresAuthRepository::new(pool)),
        }
    }

    /// Builds a development repository over a file-backed SQLite database.
    ///
    /// The server composition root never selects this constructor; it exists for
    /// backend contract tests and future SQLite enablement in S6.
    #[cfg(feature = "sqlite-backend")]
    #[must_use]
    pub fn from_sqlite(database: Arc<SqliteDatabase>) -> Self {
        Self {
            backend: Backend::Sqlite(SqliteAuthRepository::new(database)),
        }
    }

    pub async fn find_login_user(&self, email: &str) -> Result<Option<LoginUser>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.find_login_user(email).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.find_login_user(email).await,
        }
    }

    pub async fn validate_console_identity(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        auth_version: i64,
    ) -> Result<Option<LiveConsoleIdentity>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .validate_console_identity(user_id, session_id, auth_version)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .validate_console_identity(user_id, session_id, auth_version)
                    .await
            }
        }
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
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .create_session(
                        id,
                        user_id,
                        refresh_token_hash,
                        expires_at,
                        user_agent,
                        purpose,
                    )
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .create_session(
                        id,
                        user_id,
                        refresh_token_hash,
                        expires_at,
                        user_agent,
                        purpose,
                    )
                    .await
            }
        }
    }

    pub async fn password_user(
        &self,
        user_id: Uuid,
    ) -> Result<Option<PasswordUser>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.password_user(user_id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.password_user(user_id).await,
        }
    }

    pub async fn rotate_session(
        &self,
        session_id: Uuid,
        presented_hash: &[u8],
        next_hash: &[u8],
        next_expires_at: DateTime<Utc>,
        user_agent: Option<&str>,
    ) -> Result<SessionRotation, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .rotate_session(
                        session_id,
                        presented_hash,
                        next_hash,
                        next_expires_at,
                        user_agent,
                    )
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .rotate_session(
                        session_id,
                        presented_hash,
                        next_hash,
                        next_expires_at,
                        user_agent,
                    )
                    .await
            }
        }
    }

    pub async fn revoke_session_for_user(
        &self,
        user_id: Uuid,
        session_id: Uuid,
    ) -> Result<bool, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .revoke_session_for_user(user_id, session_id)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .revoke_session_for_user(user_id, session_id)
                    .await
            }
        }
    }

    pub async fn revoke_all_sessions(&self, user_id: Uuid) -> Result<(), RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.revoke_all_sessions(user_id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.revoke_all_sessions(user_id).await,
        }
    }

    pub async fn revoke_other_sessions(
        &self,
        user_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<u64, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .revoke_other_sessions(user_id, current_session_id)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .revoke_other_sessions(user_id, current_session_id)
                    .await
            }
        }
    }

    pub async fn sessions_for_user(
        &self,
        user_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<Vec<ConsoleSession>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .sessions_for_user(user_id, current_session_id)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .sessions_for_user(user_id, current_session_id)
                    .await
            }
        }
    }

    pub async fn profile(&self, user_id: Uuid) -> Result<Option<ConsoleProfile>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.profile(user_id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.profile(user_id).await,
        }
    }

    pub async fn update_display_name(
        &self,
        user_id: Uuid,
        display_name: &str,
    ) -> Result<Option<ConsoleProfile>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository.update_display_name(user_id, display_name).await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository.update_display_name(user_id, display_name).await
            }
        }
    }

    pub async fn registration_invitation_codes(
        &self,
    ) -> Result<Vec<RegistrationInvitationCode>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.registration_invitation_codes().await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.registration_invitation_codes().await,
        }
    }

    pub async fn registration_invitation_code(
        &self,
        id: Uuid,
    ) -> Result<Option<RegistrationInvitationCode>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.registration_invitation_code(id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.registration_invitation_code(id).await,
        }
    }

    pub async fn change_password(
        &self,
        user_id: Uuid,
        password_hash: &str,
    ) -> Result<bool, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository.change_password(user_id, password_hash).await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.change_password(user_id, password_hash).await,
        }
    }

    pub async fn issue_temporary_password(
        &self,
        actor_user_id: Uuid,
        actor_auth_version: i64,
        user_id: Uuid,
        password_hash: &str,
        temporary_password_ttl: Duration,
    ) -> Result<TemporaryPasswordCreated, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .issue_temporary_password(
                        actor_user_id,
                        actor_auth_version,
                        user_id,
                        password_hash,
                        temporary_password_ttl,
                    )
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .issue_temporary_password(
                        actor_user_id,
                        actor_auth_version,
                        user_id,
                        password_hash,
                        temporary_password_ttl,
                    )
                    .await
            }
        }
    }

    pub async fn complete_temporary_password(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        expected_auth_version: i64,
        password_hash: &str,
    ) -> Result<Option<SessionUser>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .complete_temporary_password(
                        user_id,
                        session_id,
                        expected_auth_version,
                        password_hash,
                    )
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .complete_temporary_password(
                        user_id,
                        session_id,
                        expected_auth_version,
                        password_hash,
                    )
                    .await
            }
        }
    }

    pub async fn reset_active_admin_password(
        &self,
        email: &str,
        password_hash: &str,
    ) -> Result<bool, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .reset_active_admin_password(email, password_hash)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .reset_active_admin_password(email, password_hash)
                    .await
            }
        }
    }

    pub async fn create_registration_invitation_code(
        &self,
        actor_user_id: Uuid,
        code_hash: &[u8],
        input: RegistrationInvitationCodeInput,
    ) -> Result<RegistrationInvitationCodeMutation, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .create_registration_invitation_code(actor_user_id, code_hash, input)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .create_registration_invitation_code(actor_user_id, code_hash, input)
                    .await
            }
        }
    }

    pub async fn update_registration_invitation_code(
        &self,
        actor_user_id: Uuid,
        id: Uuid,
        input: RegistrationInvitationCodeInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<RegistrationInvitationCodeMutation, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .update_registration_invitation_code(
                        actor_user_id,
                        id,
                        input,
                        expected_updated_at,
                    )
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .update_registration_invitation_code(
                        actor_user_id,
                        id,
                        input,
                        expected_updated_at,
                    )
                    .await
            }
        }
    }

    pub async fn register_with_invitation_code(
        &self,
        code_hash: &[u8],
        email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<RegistrationAttempt, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .register_with_invitation_code(code_hash, email, display_name, password_hash)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .register_with_invitation_code(code_hash, email, display_name, password_hash)
                    .await
            }
        }
    }

    pub async fn invite_user(
        &self,
        actor_user_id: Uuid,
        input: InviteUserInput,
        invitation_id: Uuid,
        invitation_token_hash: &[u8],
        invitation_ttl: Duration,
    ) -> Result<InvitationCreated, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .invite_user(
                        actor_user_id,
                        input,
                        invitation_id,
                        invitation_token_hash,
                        invitation_ttl,
                    )
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .invite_user(
                        actor_user_id,
                        input,
                        invitation_id,
                        invitation_token_hash,
                        invitation_ttl,
                    )
                    .await
            }
        }
    }

    pub async fn reissue_invitation(
        &self,
        actor_user_id: Uuid,
        user_id: Uuid,
        invitation_id: Uuid,
        invitation_token_hash: &[u8],
        invitation_ttl: Duration,
    ) -> Result<InvitationCreated, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .reissue_invitation(
                        actor_user_id,
                        user_id,
                        invitation_id,
                        invitation_token_hash,
                        invitation_ttl,
                    )
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .reissue_invitation(
                        actor_user_id,
                        user_id,
                        invitation_id,
                        invitation_token_hash,
                        invitation_ttl,
                    )
                    .await
            }
        }
    }

    pub async fn accept_invitation(
        &self,
        invitation_id: Uuid,
        presented_token_hash: &[u8],
        password_hash: &str,
    ) -> Result<Option<SessionUser>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .accept_invitation(invitation_id, presented_token_hash, password_hash)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .accept_invitation(invitation_id, presented_token_hash, password_hash)
                    .await
            }
        }
    }

    pub async fn bootstrap_admin(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<Uuid, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository
                    .bootstrap_admin(email, display_name, password_hash)
                    .await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository
                    .bootstrap_admin(email, display_name, password_hash)
                    .await
            }
        }
    }
}
