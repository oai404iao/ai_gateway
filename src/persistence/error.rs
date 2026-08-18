//! Backend-neutral persistence failure contract.

use std::{
    error::Error as StdError,
    fmt::{self, Debug, Display, Formatter},
};

use thiserror::Error;
use uuid::Uuid;

/// Opaque source retained for persistence failures.
///
/// Persistence adapters preserve the concrete backend error privately, but
/// this wrapper is the terminal standard error source. Its formatting and
/// source chain never expose the concrete value.
pub struct RepositoryErrorSource {
    _source: Box<dyn StdError + Send + Sync + 'static>,
}

impl RepositoryErrorSource {
    pub(crate) fn new(source: impl StdError + Send + Sync + 'static) -> Self {
        Self {
            _source: Box::new(source),
        }
    }
}

impl Debug for RepositoryErrorSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RepositoryErrorSource")
            .finish_non_exhaustive()
    }
}

impl Display for RepositoryErrorSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("opaque persistence backend failure")
    }
}

impl StdError for RepositoryErrorSource {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        None
    }
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository transaction conflicted with another transaction")]
    TransactionConflict(#[source] RepositoryErrorSource),
    #[error("repository constraint rejected the operation")]
    Constraint(#[source] RepositoryErrorSource),
    #[error("repository storage is busy")]
    Busy(#[source] RepositoryErrorSource),
    #[error("repository operation timed out")]
    Timeout(#[source] RepositoryErrorSource),
    #[error("repository and transaction belong to different backends or pool identities")]
    BackendMismatch,
    #[error("repository data or storage is corrupt")]
    Corrupt(#[source] RepositoryErrorSource),
    #[error("repository storage is unavailable")]
    StorageUnavailable(#[source] RepositoryErrorSource),
    #[error("repository migration failed")]
    Migration(#[source] RepositoryErrorSource),
    #[error("repository database operation failed")]
    DatabaseFailure(#[source] RepositoryErrorSource),
    #[error("request log response status is outside the HTTP range")]
    InvalidResponseStatus { status: u16 },
    #[error("request log id already exists with different immutable facts")]
    DuplicateConflict { id: Uuid },
    #[error("request log duplicate disappeared before it could be compared")]
    DuplicateDisappeared { id: Uuid },
    #[error("request log settlement claim became ineligible before account updates")]
    SettlementClaimInvalidated { id: Uuid },
    #[error("requested record was not found or cannot be changed")]
    NotFound,
    #[error("management record version conflicts with the current version")]
    Conflict,
    #[error("management input is invalid")]
    Validation,
    #[error("the built-in user group is protected")]
    ProtectedUserGroup,
    #[error("the user group still has members")]
    UserGroupInUse,
    #[error("the proxy is still assigned to a channel or pending OAuth flow")]
    ProxyInUse,
    #[error("an administrator cannot delete their own account")]
    CannotDeleteSelf,
    #[error("the last active administrator cannot be deleted")]
    LastAdministrator,
    #[error("an administrator cannot disable their own account in a batch")]
    CannotDisableSelf,
    #[error("an administrator cannot reset their own password through the Console")]
    CannotResetSelf,
    #[error("the selected user cannot receive a temporary password")]
    TemporaryPasswordUnavailable,
    #[error("the user has no default API key policy")]
    DefaultApiKeyPolicyRequired,
    #[error("the user's default API key policy is disabled")]
    DefaultApiKeyPolicyDisabled,
    #[error("the selected API key target is not allowed by the user's policy")]
    ApiKeyTargetNotAllowed,
    #[error("the registration invitation code name or secret already exists")]
    RegistrationInvitationCodeConflict,
    #[error("the MCP server slug already exists")]
    McpServerSlugConflict,
}
