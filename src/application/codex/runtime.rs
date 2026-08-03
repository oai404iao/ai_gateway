use std::{collections::HashMap, sync::Arc};

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use crate::persistence::CodexCredentialRecord;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexCredentialStatus {
    Active,
    Draining,
    Unavailable,
    Disabled,
}

impl CodexCredentialStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "draining" => Some(Self::Draining),
            "unavailable" => Some(Self::Unavailable),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct CompiledCodexCredential {
    credential_id: Uuid,
    account_id: Option<Arc<str>>,
    access_token: Arc<SecretString>,
    is_fedramp: bool,
    access_token_expires_at: Option<DateTime<Utc>>,
    refresh_generation: i64,
    status: CodexCredentialStatus,
}

impl std::fmt::Debug for CompiledCodexCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompiledCodexCredential")
            .field("credential_id", &self.credential_id)
            .field("account_id", &self.account_id)
            .field("access_token", &"REDACTED")
            .field("is_fedramp", &self.is_fedramp)
            .field("access_token_expires_at", &self.access_token_expires_at)
            .field("refresh_generation", &self.refresh_generation)
            .field("status", &self.status)
            .finish()
    }
}

impl CompiledCodexCredential {
    #[must_use]
    pub fn credential_id(&self) -> Uuid {
        self.credential_id
    }

    #[must_use]
    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }

    #[must_use]
    pub fn access_token(&self) -> &str {
        self.access_token.expose_secret()
    }

    #[must_use]
    pub const fn is_fedramp(&self) -> bool {
        self.is_fedramp
    }

    #[must_use]
    pub const fn refresh_generation(&self) -> i64 {
        self.refresh_generation
    }

    #[must_use]
    pub fn access_token_expired(&self) -> bool {
        self.access_token_expires_at
            .is_some_and(|expires_at| expires_at <= Utc::now())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexCredentialUnavailable {
    Missing,
    Draining,
    Unavailable,
    Disabled,
    Expired,
}

#[derive(Clone)]
pub struct CodexCredentialRuntime {
    snapshot: Arc<ArcSwap<HashMap<Uuid, Arc<CompiledCodexCredential>>>>,
}

impl Default for CodexCredentialRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexCredentialRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(HashMap::new())),
        }
    }

    pub fn replace(&self, records: Vec<CodexCredentialRecord>) {
        let mut credentials = HashMap::new();
        for record in records {
            let Some(status) = CodexCredentialStatus::parse(&record.runtime_status) else {
                continue;
            };
            let projection_channel_ids = if record.projection_channel_ids.is_empty() {
                vec![record.channel_id]
            } else {
                record.projection_channel_ids.clone()
            };
            let credential = Arc::new(CompiledCodexCredential {
                credential_id: record.channel_id,
                account_id: record.account_id.map(Arc::from),
                access_token: Arc::new(SecretString::from(record.access_token)),
                is_fedramp: record.is_fedramp,
                access_token_expires_at: record.access_token_expires_at,
                refresh_generation: record.refresh_generation,
                status,
            });
            for channel_id in projection_channel_ids {
                credentials.insert(channel_id, Arc::clone(&credential));
            }
        }
        self.snapshot.store(Arc::new(credentials));
    }

    pub fn credential(
        &self,
        channel_id: Uuid,
        affinity_cache_hit: bool,
    ) -> Result<Arc<CompiledCodexCredential>, CodexCredentialUnavailable> {
        let snapshot = self.snapshot.load();
        let credential = snapshot
            .get(&channel_id)
            .cloned()
            .ok_or(CodexCredentialUnavailable::Missing)?;
        match credential.status {
            CodexCredentialStatus::Disabled => Err(CodexCredentialUnavailable::Disabled),
            CodexCredentialStatus::Unavailable => Err(CodexCredentialUnavailable::Unavailable),
            CodexCredentialStatus::Draining if !affinity_cache_hit => {
                Err(CodexCredentialUnavailable::Draining)
            }
            CodexCredentialStatus::Active | CodexCredentialStatus::Draining => {
                if credential.access_token_expired() {
                    Err(CodexCredentialUnavailable::Expired)
                } else {
                    Ok(credential)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn record(status: &str, expires_at: Option<DateTime<Utc>>) -> CodexCredentialRecord {
        let now = Utc::now();
        CodexCredentialRecord {
            channel_id: Uuid::from_u128(1),
            channel_group_id: Uuid::from_u128(2),
            connector_pool_id: Uuid::from_u128(2),
            projection_channel_ids: vec![Uuid::from_u128(1), Uuid::from_u128(3)],
            label: "credential".into(),
            email: Some("codex@example.test".into()),
            account_id: Some("account-123".into()),
            user_id: Some("user-123".into()),
            plan_type: Some("plus".into()),
            is_fedramp: false,
            id_token: "id-token".into(),
            access_token: "access-token".into(),
            refresh_token: "refresh-token".into(),
            access_token_expires_at: expires_at,
            last_refreshed_at: now,
            refresh_generation: 7,
            reauth_required: false,
            quota_threshold_percent: 95,
            runtime_status: status.into(),
            quota_allowed: Some(true),
            quota_limit_reached: Some(false),
            primary_used_percent: Some(50),
            primary_window_seconds: Some(10_800),
            primary_reset_at: Some(now + Duration::hours(1)),
            secondary_used_percent: None,
            secondary_window_seconds: None,
            secondary_reset_at: None,
            quota_reset_credits_available: None,
            quota_checked_at: Some(now),
            last_error_code: None,
            last_error_summary: None,
            proxy_id: None,
            weight: 100,
            enabled: true,
            available_models: vec!["gpt-5-codex".into()],
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn draining_credentials_are_available_only_to_existing_affinity_sessions() {
        let runtime = CodexCredentialRuntime::new();
        runtime.replace(vec![record(
            "draining",
            Some(Utc::now() + Duration::hours(1)),
        )]);

        assert_eq!(
            runtime.credential(Uuid::from_u128(1), false).unwrap_err(),
            CodexCredentialUnavailable::Draining
        );
        let credential = runtime.credential(Uuid::from_u128(1), true).unwrap();
        assert_eq!(credential.account_id(), Some("account-123"));
        assert_eq!(credential.refresh_generation(), 7);
    }

    #[test]
    fn expired_tokens_are_unavailable_even_when_the_status_is_active() {
        let runtime = CodexCredentialRuntime::new();
        runtime.replace(vec![record(
            "active",
            Some(Utc::now() - Duration::seconds(1)),
        )]);

        assert_eq!(
            runtime.credential(Uuid::from_u128(1), true).unwrap_err(),
            CodexCredentialUnavailable::Expired
        );
    }

    #[test]
    fn explicit_credential_state_takes_precedence_over_token_expiration() {
        let runtime = CodexCredentialRuntime::new();
        runtime.replace(vec![record(
            "disabled",
            Some(Utc::now() - Duration::seconds(1)),
        )]);

        assert_eq!(
            runtime.credential(Uuid::from_u128(1), true).unwrap_err(),
            CodexCredentialUnavailable::Disabled
        );
    }

    #[test]
    fn debug_output_redacts_access_tokens() {
        let runtime = CodexCredentialRuntime::new();
        runtime.replace(vec![record("active", None)]);
        let credential = runtime.credential(Uuid::from_u128(1), false).unwrap();
        let images_projection = runtime.credential(Uuid::from_u128(3), false).unwrap();
        let debug = format!("{credential:?}");

        assert_eq!(images_projection.credential_id(), Uuid::from_u128(1));
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("access-token"));
    }
}
