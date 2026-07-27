//! Console login, JWT issuance, refresh-session rotation, and invitation flows.

use std::{
    collections::HashMap,
    fmt::Write as _,
    fs,
    sync::Arc,
    time::{Duration, Instant},
};

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    domain::{ConsolePrincipal, UserRole},
    persistence::{
        AuthRepository, InvitationCreated, InviteUserInput, LoginUser, RegistrationAttempt,
        RegistrationInvitationCodeInput as PersistenceRegistrationCodeInput,
        RegistrationInvitationCodeMutation as PersistenceRegistrationCodeMutation, RepositoryError,
        SessionRotation, SessionUser,
    },
    runtime_config::AuthConfig,
};

const MIN_PASSWORD_BYTES: usize = 12;
const MAX_PASSWORD_BYTES: usize = 1_024;
const MAX_EMAIL_BYTES: usize = 320;
const MAX_DISPLAY_NAME_BYTES: usize = 200;
const MIN_REGISTRATION_CODE_BYTES: usize = 12;
const MAX_REGISTRATION_CODE_BYTES: usize = 128;
const MAX_REGISTRATION_CODE_NAME_BYTES: usize = 100;
const MAX_SESSION_USER_AGENT_CHARS: usize = 512;
const INVITATION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const AUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const AUTH_FAILURE_LIMIT: u32 = 10;

/// The Console authentication use case. Its state is separate from API-key
/// authentication in the data plane and only serves Console HTTP routes.
#[derive(Clone)]
pub struct ConsoleAuthService {
    repository: AuthRepository,
    tokens: Arc<TokenCodec>,
    failures: Arc<Mutex<AuthFailureLimiter>>,
}

impl ConsoleAuthService {
    pub fn from_config(repository: AuthRepository, config: &AuthConfig) -> Result<Self, AuthError> {
        Ok(Self {
            repository,
            tokens: Arc::new(TokenCodec::from_config(config)?),
            failures: Arc::new(Mutex::new(AuthFailureLimiter::default())),
        })
    }

    /// Builds the same runtime from already-loaded PEM bytes. This supports
    /// secret-manager integrations and deterministic integration tests without
    /// weakening the file-path configuration boundary used by the binary.
    pub fn from_pem(
        repository: AuthRepository,
        config: &AuthConfig,
        signing_key_pem: &[u8],
        verification_key_pem: &[u8],
    ) -> Result<Self, AuthError> {
        Ok(Self {
            repository,
            tokens: Arc::new(TokenCodec::from_pem(
                config,
                signing_key_pem,
                verification_key_pem,
            )?),
            failures: Arc::new(Mutex::new(AuthFailureLimiter::default())),
        })
    }

    #[must_use]
    pub fn repository(&self) -> &AuthRepository {
        &self.repository
    }

    pub async fn login(&self, email: String, password: String) -> Result<IssuedSession, AuthError> {
        self.login_with_user_agent(email, password, None).await
    }

    pub async fn login_with_user_agent(
        &self,
        email: String,
        password: String,
        user_agent: Option<String>,
    ) -> Result<IssuedSession, AuthError> {
        validate_email(&email)?;
        validate_password(&password)?;
        let limiter_key = format!("login:{}", email.trim().to_ascii_lowercase());
        self.ensure_attempt_allowed(&limiter_key).await?;
        let user = match self.repository.find_login_user(email.trim()).await? {
            Some(user) => user,
            None => {
                self.record_failed_attempt(&limiter_key).await;
                return Err(AuthError::InvalidCredentials);
            }
        };
        let user = match active_login_user(user) {
            Ok(user) => user,
            Err(error) => {
                self.record_failed_attempt(&limiter_key).await;
                return Err(error);
            }
        };
        if !verify_password(password, user.password_hash.clone()).await? {
            self.record_failed_attempt(&limiter_key).await;
            return Err(AuthError::InvalidCredentials);
        }
        self.clear_failed_attempts(&limiter_key).await;
        self.issue_session(user.into_session_user(), user_agent)
            .await
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<IssuedSession, AuthError> {
        self.refresh_with_user_agent(refresh_token, None).await
    }

    pub async fn refresh_with_user_agent(
        &self,
        refresh_token: &str,
        user_agent: Option<String>,
    ) -> Result<IssuedSession, AuthError> {
        let user_agent = normalize_session_user_agent(user_agent);
        let (session_id, _) = parse_opaque_token(refresh_token).ok_or(AuthError::InvalidToken)?;
        let next = new_opaque_token(session_id);
        let next_hash = token_hash(&next);
        let next_expiry = self.refresh_expiry()?;
        let user = match self
            .repository
            .rotate_session(
                session_id,
                &token_hash(refresh_token),
                &next_hash,
                next_expiry,
                user_agent.as_deref(),
            )
            .await?
        {
            SessionRotation::Rotated(user) => user,
            SessionRotation::Invalid | SessionRotation::Replayed => {
                return Err(AuthError::InvalidToken);
            }
        };
        self.finish_issued_session(user, session_id, next, next_expiry)
    }

    pub async fn authenticate_access_token(
        &self,
        token: &str,
    ) -> Result<ConsolePrincipal, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::InvalidToken)?;
        if header.alg != Algorithm::EdDSA || header.kid.as_deref() != Some(&self.tokens.key_id) {
            return Err(AuthError::InvalidToken);
        }
        let decoded =
            decode::<AccessClaims>(token, &self.tokens.decoding_key, &self.tokens.validation)
                .map_err(|_| AuthError::InvalidToken)?;
        let claims = decoded.claims;
        let identity = self
            .repository
            .validate_console_identity(claims.sub, claims.sid, claims.auth_version)
            .await?
            .ok_or(AuthError::InvalidToken)?;
        let role = UserRole::parse(&identity.role).ok_or(AuthError::InvalidToken)?;
        if claims.role != role || identity.auth_version != claims.auth_version {
            return Err(AuthError::InvalidToken);
        }
        Ok(ConsolePrincipal::new(
            identity.user_id,
            identity.session_id,
            role,
            identity.auth_version,
        ))
    }

    pub async fn logout(&self, principal: ConsolePrincipal) -> Result<(), AuthError> {
        self.repository
            .revoke_session_for_user(principal.user_id(), principal.session_id())
            .await?;
        Ok(())
    }

    pub async fn accept_invitation(
        &self,
        invitation_token: &str,
        password: String,
    ) -> Result<IssuedSession, AuthError> {
        self.accept_invitation_with_user_agent(invitation_token, password, None)
            .await
    }

    pub async fn accept_invitation_with_user_agent(
        &self,
        invitation_token: &str,
        password: String,
        user_agent: Option<String>,
    ) -> Result<IssuedSession, AuthError> {
        validate_password(&password)?;
        let (invitation_id, _) =
            parse_opaque_token(invitation_token).ok_or(AuthError::InvalidInvitation)?;
        let limiter_key = format!("invitation:{invitation_id}");
        self.ensure_attempt_allowed(&limiter_key).await?;
        let password_hash = hash_console_password(password).await?;
        let user = match self
            .repository
            .accept_invitation(invitation_id, &token_hash(invitation_token), &password_hash)
            .await?
        {
            Some(user) => user,
            None => {
                self.record_failed_attempt(&limiter_key).await;
                return Err(AuthError::InvalidInvitation);
            }
        };
        self.clear_failed_attempts(&limiter_key).await;
        self.issue_session(user, user_agent).await
    }

    pub async fn register(&self, input: SelfRegistrationInput) -> Result<IssuedSession, AuthError> {
        self.register_with_user_agent(input, None).await
    }

    pub async fn register_with_user_agent(
        &self,
        input: SelfRegistrationInput,
        user_agent: Option<String>,
    ) -> Result<IssuedSession, AuthError> {
        validate_email(&input.email)?;
        validate_display_name(&input.display_name)?;
        validate_password(&input.password)?;
        let invitation_code = normalize_registration_code(&input.invitation_code)?;
        let code_hash = token_hash(&invitation_code);
        let email_limiter_key = format!("registration:{}", input.email.trim().to_ascii_lowercase());
        let code_limiter_key = format!("registration-code:{}", limiter_hash_prefix(&code_hash));
        self.ensure_attempt_allowed(&email_limiter_key).await?;
        self.ensure_attempt_allowed(&code_limiter_key).await?;

        let password_hash = hash_console_password(input.password).await?;
        let registration = self
            .repository
            .register_with_invitation_code(
                &code_hash,
                input.email.trim(),
                input.display_name.trim(),
                &password_hash,
            )
            .await?;
        let user = match registration {
            RegistrationAttempt::Registered(user) => user,
            RegistrationAttempt::InvalidCode => {
                self.record_failed_attempt(&email_limiter_key).await;
                self.record_failed_attempt(&code_limiter_key).await;
                return Err(AuthError::InvalidRegistrationCode);
            }
            RegistrationAttempt::EmailConflict => {
                self.record_failed_attempt(&email_limiter_key).await;
                self.record_failed_attempt(&code_limiter_key).await;
                return Err(AuthError::RegistrationConflict);
            }
        };
        self.clear_failed_attempts(&email_limiter_key).await;
        self.clear_failed_attempts(&code_limiter_key).await;
        self.issue_session(user, user_agent).await
    }

    pub async fn change_password(
        &self,
        principal: ConsolePrincipal,
        current_password: String,
        next_password: String,
    ) -> Result<(), AuthError> {
        validate_password(&current_password)?;
        validate_password(&next_password)?;
        let limiter_key = format!("password:{}", principal.user_id());
        self.ensure_attempt_allowed(&limiter_key).await?;
        let user = self
            .repository
            .password_user(principal.user_id())
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        if user.status != "active" || !verify_password(current_password, user.password_hash).await?
        {
            self.record_failed_attempt(&limiter_key).await;
            return Err(AuthError::InvalidCredentials);
        }
        let password_hash = hash_console_password(next_password).await?;
        if !self
            .repository
            .change_password(principal.user_id(), &password_hash)
            .await?
        {
            self.record_failed_attempt(&limiter_key).await;
            return Err(AuthError::InvalidCredentials);
        }
        self.clear_failed_attempts(&limiter_key).await;
        Ok(())
    }

    pub async fn invite_user(
        &self,
        actor: ConsolePrincipal,
        input: InviteUserInput,
    ) -> Result<IssuedInvitation, AuthError> {
        if !actor.role().is_admin() {
            return Err(AuthError::Forbidden);
        }
        validate_email(&input.email)?;
        validate_display_name(&input.display_name)?;
        if input.initial_balance_amount.is_sign_negative() {
            return Err(AuthError::InvalidInput);
        }
        let invitation_id = Uuid::new_v4();
        // The ID in the transport token is intentionally also the invitation
        // primary key, so acceptance can locate one indexed row before doing a
        // constant-time secret comparison.
        let token = new_opaque_token(invitation_id);
        let created = match self
            .repository
            .invite_user(
                actor.user_id(),
                input,
                invitation_id,
                &token_hash(&token),
                INVITATION_TTL,
            )
            .await
        {
            Ok(created) => created,
            Err(RepositoryError::NotFound) => return Err(AuthError::Forbidden),
            Err(error) => return Err(error.into()),
        };
        Ok(IssuedInvitation { created, token })
    }

    pub async fn reissue_invitation(
        &self,
        actor: ConsolePrincipal,
        user_id: Uuid,
    ) -> Result<IssuedInvitation, AuthError> {
        if !actor.role().is_admin() {
            return Err(AuthError::Forbidden);
        }
        let invitation_id = Uuid::new_v4();
        let token = new_opaque_token(invitation_id);
        let created = match self
            .repository
            .reissue_invitation(
                actor.user_id(),
                user_id,
                invitation_id,
                &token_hash(&token),
                INVITATION_TTL,
            )
            .await
        {
            Ok(created) => created,
            Err(RepositoryError::NotFound) => return Err(AuthError::NotFound),
            Err(RepositoryError::Validation) => return Err(AuthError::InvalidInput),
            Err(error) => return Err(error.into()),
        };
        Ok(IssuedInvitation { created, token })
    }

    pub async fn create_registration_invitation_code(
        &self,
        actor: ConsolePrincipal,
        input: RegistrationInvitationCodeCreateInput,
    ) -> Result<IssuedRegistrationInvitationCode, AuthError> {
        if !actor.role().is_admin() {
            return Err(AuthError::Forbidden);
        }
        validate_registration_invitation_code_name(&input.name)?;
        validate_registration_invitation_code_settings(
            input.max_uses,
            input.initial_balance_amount,
        )?;
        let invitation_code = normalize_registration_code(&input.invitation_code)?;
        let mutation = self
            .repository
            .create_registration_invitation_code(
                actor.user_id(),
                &token_hash(&invitation_code),
                PersistenceRegistrationCodeInput {
                    name: input.name,
                    max_uses: input.max_uses,
                    expires_at: input.expires_at,
                    enabled: input.enabled,
                    user_group_id: input.user_group_id,
                    initial_balance_amount: input.initial_balance_amount,
                },
            )
            .await?;
        Ok(IssuedRegistrationInvitationCode {
            id: mutation.id,
            invitation_code,
            correlation_id: mutation.correlation_id,
        })
    }

    pub async fn update_registration_invitation_code(
        &self,
        actor: ConsolePrincipal,
        id: Uuid,
        input: RegistrationInvitationCodeUpdateInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<RegistrationInvitationCodeMutation, AuthError> {
        if !actor.role().is_admin() {
            return Err(AuthError::Forbidden);
        }
        validate_registration_invitation_code_name(&input.name)?;
        validate_registration_invitation_code_settings(
            input.max_uses,
            input.initial_balance_amount,
        )?;
        let mutation = self
            .repository
            .update_registration_invitation_code(
                actor.user_id(),
                id,
                PersistenceRegistrationCodeInput {
                    name: input.name,
                    max_uses: input.max_uses,
                    expires_at: input.expires_at,
                    enabled: input.enabled,
                    user_group_id: input.user_group_id,
                    initial_balance_amount: input.initial_balance_amount,
                },
                expected_updated_at,
            )
            .await?;
        Ok(mutation.into())
    }

    pub async fn bootstrap_admin(
        &self,
        email: String,
        display_name: String,
        password: String,
    ) -> Result<Uuid, AuthError> {
        validate_email(&email)?;
        validate_display_name(&display_name)?;
        validate_password(&password)?;
        let password_hash = hash_console_password(password).await?;
        Ok(self
            .repository
            .bootstrap_admin(&email, &display_name, &password_hash)
            .await?)
    }

    async fn ensure_attempt_allowed(&self, key: &str) -> Result<(), AuthError> {
        if self.failures.lock().await.allows(key) {
            Ok(())
        } else {
            Err(AuthError::RateLimited)
        }
    }

    async fn record_failed_attempt(&self, key: &str) {
        self.failures.lock().await.record_failure(key);
    }

    async fn clear_failed_attempts(&self, key: &str) {
        self.failures.lock().await.clear(key);
    }

    async fn issue_session(
        &self,
        user: SessionUser,
        user_agent: Option<String>,
    ) -> Result<IssuedSession, AuthError> {
        let user_agent = normalize_session_user_agent(user_agent);
        let session_id = Uuid::new_v4();
        let refresh_token = new_opaque_token(session_id);
        let refresh_expiry = self.refresh_expiry()?;
        self.repository
            .create_session(
                session_id,
                user.id,
                &token_hash(&refresh_token),
                refresh_expiry,
                user_agent.as_deref(),
            )
            .await?;
        self.finish_issued_session(user, session_id, refresh_token, refresh_expiry)
    }

    fn finish_issued_session(
        &self,
        user: SessionUser,
        session_id: Uuid,
        refresh_token: String,
        refresh_expires_at: DateTime<Utc>,
    ) -> Result<IssuedSession, AuthError> {
        let access_token = self.tokens.issue(&user, session_id)?;
        Ok(IssuedSession {
            access_token,
            expires_in_seconds: self.tokens.access_token_ttl_seconds,
            refresh_token,
            refresh_expires_at,
            user: ConsoleUser {
                id: user.id,
                email: user.email.unwrap_or_default(),
                display_name: user.display_name,
                role: user.role,
            },
        })
    }

    fn refresh_expiry(&self) -> Result<DateTime<Utc>, AuthError> {
        Utc::now()
            .checked_add_signed(
                chrono::Duration::from_std(Duration::from_secs(
                    self.tokens.refresh_token_ttl_seconds,
                ))
                .map_err(|_| AuthError::Configuration)?,
            )
            .ok_or(AuthError::Configuration)
    }
}

fn normalize_session_user_agent(user_agent: Option<String>) -> Option<String> {
    let user_agent = user_agent?;
    let user_agent = user_agent.trim();
    if user_agent.is_empty() {
        return None;
    }
    Some(
        user_agent
            .chars()
            .take(MAX_SESSION_USER_AGENT_CHARS)
            .collect(),
    )
}

#[derive(Clone)]
struct TokenCodec {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    validation: Validation,
    issuer: String,
    audience: String,
    key_id: String,
    access_token_ttl_seconds: u64,
    refresh_token_ttl_seconds: u64,
}

impl TokenCodec {
    fn from_config(config: &AuthConfig) -> Result<Self, AuthError> {
        let private = Zeroizing::new(
            fs::read(&config.signing_key_path).map_err(|_| AuthError::Configuration)?,
        );
        let public = Zeroizing::new(
            fs::read(&config.verification_key_path).map_err(|_| AuthError::Configuration)?,
        );
        Self::from_pem(config, &private, &public)
    }

    fn from_pem(
        config: &AuthConfig,
        signing_key_pem: &[u8],
        verification_key_pem: &[u8],
    ) -> Result<Self, AuthError> {
        let encoding_key =
            EncodingKey::from_ed_pem(signing_key_pem).map_err(|_| AuthError::Configuration)?;
        let decoding_key =
            DecodingKey::from_ed_pem(verification_key_pem).map_err(|_| AuthError::Configuration)?;
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.leeway = 5;
        validation.set_issuer(&[&config.issuer]);
        validation.set_audience(&[&config.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        Ok(Self {
            encoding_key,
            decoding_key,
            validation,
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
            key_id: config.key_id.clone(),
            access_token_ttl_seconds: config.access_token_ttl_seconds,
            refresh_token_ttl_seconds: config.refresh_token_ttl_seconds,
        })
    }

    fn issue(&self, user: &SessionUser, session_id: Uuid) -> Result<String, AuthError> {
        let now = Utc::now().timestamp();
        let exp = now
            .checked_add(
                i64::try_from(self.access_token_ttl_seconds)
                    .map_err(|_| AuthError::Configuration)?,
            )
            .ok_or(AuthError::Configuration)?;
        let claims = AccessClaims {
            sub: user.id,
            sid: session_id,
            role: user.role,
            auth_version: user.auth_version,
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            iat: now,
            exp,
            jti: Uuid::new_v4(),
        };
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.key_id.clone());
        encode(&header, &claims, &self.encoding_key).map_err(|_| AuthError::Configuration)
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct AccessClaims {
    sub: Uuid,
    sid: Uuid,
    role: UserRole,
    auth_version: i64,
    iss: String,
    aud: String,
    iat: i64,
    exp: i64,
    jti: Uuid,
}

#[derive(Clone, Serialize)]
pub struct ConsoleUser {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
}

pub struct IssuedSession {
    pub access_token: String,
    pub expires_in_seconds: u64,
    pub refresh_token: String,
    pub refresh_expires_at: DateTime<Utc>,
    pub user: ConsoleUser,
}

pub struct IssuedInvitation {
    pub created: InvitationCreated,
    pub token: String,
}

pub struct SelfRegistrationInput {
    pub invitation_code: String,
    pub email: String,
    pub display_name: String,
    pub password: String,
}

pub struct RegistrationInvitationCodeCreateInput {
    pub name: String,
    pub invitation_code: String,
    pub max_uses: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub user_group_id: Uuid,
    pub initial_balance_amount: rust_decimal::Decimal,
}

pub struct RegistrationInvitationCodeUpdateInput {
    pub name: String,
    pub max_uses: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub user_group_id: Uuid,
    pub initial_balance_amount: rust_decimal::Decimal,
}

pub struct IssuedRegistrationInvitationCode {
    pub id: Uuid,
    pub invitation_code: String,
    pub correlation_id: Uuid,
}

pub struct RegistrationInvitationCodeMutation {
    pub id: Uuid,
    pub correlation_id: Uuid,
}

impl From<PersistenceRegistrationCodeMutation> for RegistrationInvitationCodeMutation {
    fn from(value: PersistenceRegistrationCodeMutation) -> Self {
        Self {
            id: value.id,
            correlation_id: value.correlation_id,
        }
    }
}

fn active_login_user(user: LoginUser) -> Result<LoginUser, AuthError> {
    if user.status != "active"
        || user.password_hash.is_none()
        || UserRole::parse(&user.role).is_none()
    {
        return Err(AuthError::InvalidCredentials);
    }
    Ok(user)
}

impl LoginUser {
    fn into_session_user(self) -> SessionUser {
        SessionUser {
            id: self.id,
            email: self.email,
            display_name: self.display_name,
            role: UserRole::parse(&self.role).expect("active login role was validated"),
            auth_version: self.auth_version,
        }
    }
}

fn validate_email(value: &str) -> Result<(), AuthError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_EMAIL_BYTES
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
        || !value.contains('@')
    {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn validate_display_name(value: &str) -> Result<(), AuthError> {
    if value.trim().is_empty() || value.len() > MAX_DISPLAY_NAME_BYTES {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn validate_password(value: &str) -> Result<(), AuthError> {
    if value.len() < MIN_PASSWORD_BYTES || value.len() > MAX_PASSWORD_BYTES {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn validate_registration_invitation_code_name(value: &str) -> Result<(), AuthError> {
    if value.trim().is_empty() || value.len() > MAX_REGISTRATION_CODE_NAME_BYTES {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn validate_registration_invitation_code_settings(
    max_uses: Option<i64>,
    initial_balance_amount: rust_decimal::Decimal,
) -> Result<(), AuthError> {
    if max_uses.is_some_and(|maximum| maximum <= 0) || initial_balance_amount.is_sign_negative() {
        return Err(AuthError::InvalidInput);
    }
    Ok(())
}

fn normalize_registration_code(value: &str) -> Result<String, AuthError> {
    let value = value.trim();
    if value.len() < MIN_REGISTRATION_CODE_BYTES
        || value.len() > MAX_REGISTRATION_CODE_BYTES
        || value.chars().any(char::is_whitespace)
    {
        return Err(AuthError::InvalidInput);
    }
    Ok(value.to_owned())
}

fn limiter_hash_prefix(hash: &[u8]) -> String {
    let mut prefix = String::with_capacity(16);
    for byte in hash.iter().take(8) {
        write!(&mut prefix, "{byte:02x}").expect("writing to a String cannot fail");
    }
    prefix
}

pub async fn hash_console_password(password: String) -> Result<String, AuthError> {
    validate_password(&password)?;
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| AuthError::Configuration)
    })
    .await
    .map_err(|_| AuthError::Configuration)?
}

async fn verify_password(password: String, encoded: Option<String>) -> Result<bool, AuthError> {
    let Some(encoded) = encoded else {
        return Ok(false);
    };
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&encoded).map_err(|_| AuthError::Configuration)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .map_err(|_| AuthError::Configuration)?
}

fn new_opaque_token(id: Uuid) -> String {
    // Two independent UUIDv4 values add 244 random bits after the indexed
    // record identifier. The raw value is never persisted.
    format!(
        "{id}.{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

fn parse_opaque_token(value: &str) -> Option<(Uuid, &str)> {
    let (id, secret) = value.split_once('.')?;
    if secret.is_empty() || secret.len() != 64 {
        return None;
    }
    Some((Uuid::parse_str(id).ok()?, secret))
}

fn token_hash(value: &str) -> Vec<u8> {
    Sha256::digest(value.as_bytes()).to_vec()
}

#[derive(Default)]
struct AuthFailureLimiter {
    windows: HashMap<String, AuthFailureWindow>,
}

struct AuthFailureWindow {
    started_at: Instant,
    failures: u32,
}

impl AuthFailureLimiter {
    fn allows(&mut self, key: &str) -> bool {
        self.prune();
        self.windows
            .get(key)
            .is_none_or(|window| window.failures < AUTH_FAILURE_LIMIT)
    }

    fn record_failure(&mut self, key: &str) {
        self.prune();
        let now = Instant::now();
        match self.windows.get_mut(key) {
            Some(window) => window.failures = window.failures.saturating_add(1),
            None => {
                self.windows.insert(
                    key.to_owned(),
                    AuthFailureWindow {
                        started_at: now,
                        failures: 1,
                    },
                );
            }
        }
    }

    fn clear(&mut self, key: &str) {
        self.windows.remove(key);
    }

    fn prune(&mut self) {
        self.windows
            .retain(|_, window| window.started_at.elapsed() < AUTH_FAILURE_WINDOW);
    }
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Console authentication configuration is invalid")]
    Configuration,
    #[error("invalid login credentials")]
    InvalidCredentials,
    #[error("invalid or expired Console token")]
    InvalidToken,
    #[error("invalid or expired invitation")]
    InvalidInvitation,
    #[error("invalid, expired, disabled, or exhausted registration invitation code")]
    InvalidRegistrationCode,
    #[error("a Console account already uses this email")]
    RegistrationConflict,
    #[error("invalid authentication input")]
    InvalidInput,
    #[error("too many failed authentication attempts")]
    RateLimited,
    #[error("Console action is forbidden")]
    Forbidden,
    #[error("Console authentication record was not found")]
    NotFound,
    #[error("Console authentication storage failed")]
    Repository(#[from] RepositoryError),
}

#[cfg(test)]
mod tests {
    use super::{AuthError, hash_console_password, normalize_session_user_agent};

    #[tokio::test]
    async fn hashing_a_console_password_rejects_a_short_value() {
        assert!(matches!(
            hash_console_password("too-short".to_owned()).await,
            Err(AuthError::InvalidInput)
        ));
    }

    #[test]
    fn session_user_agent_is_trimmed_and_bounded() {
        assert_eq!(
            normalize_session_user_agent(Some("  Browser/1.0  ".into())).as_deref(),
            Some("Browser/1.0")
        );
        assert_eq!(normalize_session_user_agent(Some("   ".into())), None);
        assert_eq!(
            normalize_session_user_agent(Some("x".repeat(600)))
                .unwrap()
                .chars()
                .count(),
            512
        );
    }
}
