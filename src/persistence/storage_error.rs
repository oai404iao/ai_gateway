//! Backend failure classification without exposing driver types to callers.

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageFailureKind {
    Conflict,
    InvalidInput,
    RoutingDependency,
    Internal,
}

#[derive(Debug, Error)]
#[error("storage operation failed")]
pub struct StorageError {
    kind: StorageFailureKind,
    #[source]
    source: sqlx::Error,
}

impl StorageError {
    #[must_use]
    pub fn kind(&self) -> StorageFailureKind {
        self.kind
    }
}

impl From<sqlx::Error> for super::RepositoryError {
    fn from(source: sqlx::Error) -> Self {
        let kind = source
            .as_database_error()
            .map_or(StorageFailureKind::Internal, |error| {
                if matches!(error.code().as_deref(), Some("40001" | "40P01")) {
                    StorageFailureKind::Conflict
                } else if error.constraint().is_some_and(|constraint| {
                    matches!(
                        constraint,
                        "channels_channel_group_id_api_format_fkey"
                            | "channels_proxy_id_fkey"
                            | "channels_config_template_id_fkey"
                            | "model_rule_tiers_rule_format_fk"
                            | "model_rule_groups_tier_fk"
                            | "model_rule_groups_group_format_fk"
                            | "model_rule_channels_group_target_fk"
                            | "model_rule_channels_channel_group_format_fk"
                    )
                }) {
                    StorageFailureKind::RoutingDependency
                } else if matches!(
                    error.code().as_deref(),
                    Some("22001" | "22007" | "22P02" | "23502" | "23503" | "23505" | "23514")
                ) {
                    StorageFailureKind::InvalidInput
                } else {
                    StorageFailureKind::Internal
                }
            });
        Self::Storage(StorageError { kind, source })
    }
}
