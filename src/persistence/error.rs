//! Backend-neutral persistence failure contract.

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("control-plane database operation failed")]
    Sql(#[from] sqlx::Error),
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
