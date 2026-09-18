//! Internal-failure classification for operations outside a development backend's supported slice.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendKind {
    Postgres,
    Sqlite,
}

impl BackendKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A packaging error, never a business validation failure or a retryable conflict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedBackendOperation {
    backend: BackendKind,
    operation: &'static str,
}

impl UnsupportedBackendOperation {
    #[must_use]
    pub const fn new(backend: BackendKind, operation: &'static str) -> Self {
        Self { backend, operation }
    }

    #[must_use]
    pub const fn backend(self) -> BackendKind {
        self.backend
    }

    #[must_use]
    pub const fn operation(self) -> &'static str {
        self.operation
    }
}

impl fmt::Display for UnsupportedBackendOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the {} backend does not implement {}",
            self.backend, self.operation
        )
    }
}

impl std::error::Error for UnsupportedBackendOperation {}

impl From<UnsupportedBackendOperation> for super::RepositoryError {
    fn from(operation: UnsupportedBackendOperation) -> Self {
        Self::from(sqlx::Error::Configuration(Box::new(operation)))
    }
}
