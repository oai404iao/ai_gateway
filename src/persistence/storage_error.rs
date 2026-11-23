//! Backend failure classification without exposing driver types to callers.

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageFailureKind {
    Conflict,
    InvalidInput,
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
    pub(super) fn create_only(error: super::RepositoryError) -> super::RepositoryError {
        match error {
            super::RepositoryError::Storage(ref storage)
                if storage
                    .source
                    .as_database_error()
                    .is_some_and(|e| e.is_unique_violation()) =>
            {
                super::RepositoryError::Conflict
            }
            other => other,
        }
    }

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
                #[cfg(feature = "sqlite-backend")]
                if error
                    .try_downcast_ref::<sqlx::sqlite::SqliteError>()
                    .is_some()
                {
                    return classify_sqlite(error);
                }
                if matches!(error.code().as_deref(), Some("40001" | "40P01")) {
                    StorageFailureKind::Conflict
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

#[cfg(feature = "sqlite-backend")]
fn classify_sqlite(error: &dyn sqlx::error::DatabaseError) -> StorageFailureKind {
    if error.message() == "codex_operation_pending" {
        return StorageFailureKind::Conflict;
    }
    let code = error.code().and_then(|code| code.parse::<i32>().ok());
    match code.map(|code| code & 0xff) {
        Some(5 | 6) => StorageFailureKind::Conflict,
        Some(18..=20) => StorageFailureKind::InvalidInput,
        _ if error.message() == "SQLite schema value rejected"
            || error.message() == "malformed JSON" =>
        {
            StorageFailureKind::InvalidInput
        }
        _ => StorageFailureKind::Internal,
    }
}
