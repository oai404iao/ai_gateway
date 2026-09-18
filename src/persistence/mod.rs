//! Repository operations with explicit backend dispatch; SQLite is not yet selectable in production.

mod auth;
pub(crate) mod backend;
mod backend_auth;
mod backend_control_plane;
mod codex;
mod codex_sharing;
mod codex_write;
mod control_plane_write;
mod health;
mod metering;
mod migrations;
mod postgres_control_plane;
#[cfg(feature = "sqlite-backend")]
pub mod sqlite;
mod storage_error;

use auth::PostgresAuthRepository;
pub use auth::{
    ConsoleProfile, ConsoleSession, ConsoleSessionState, InvitationCreated, InviteUserInput,
    LiveConsoleIdentity, LoginUser, PasswordUser, RegistrationAttempt, RegistrationInvitationCode,
    RegistrationInvitationCodeInput, RegistrationInvitationCodeMutation, SessionRotation,
    SessionUser, TemporaryPasswordCreated,
};
pub use backend::{BackendKind, UnsupportedBackendOperation};
pub use backend_auth::AuthRepository;
pub use backend_control_plane::{ControlPlaneRepository, PreparedControlPlaneChange};
pub use codex::{
    CodexCredentialBatchInput, CodexCredentialBatchOperation, CodexCredentialBatchTarget,
    CodexCredentialCreate, CodexCredentialExportBundle, CodexCredentialExportInput,
    CodexCredentialExportItem, CodexCredentialExportProxy, CodexCredentialImportInput,
    CodexCredentialRecord, CodexCredentialUpdateInput, CodexCredentialView, CodexOauthFlowRecord,
    CodexOauthStartInput, CodexQuotaResetOutcome, CodexQuotaUpdate, CodexQuotaWindowHistory,
    CodexQuotaWindowPeriodView, CodexTokenRefreshUpdate, SelfCodexQuotaCredentialView,
    SelfCodexQuotaWindowHistory, SelfCodexQuotaWindowPeriodView,
};
pub use codex_write::{CodexQuotaReset, CodexRefresh};
pub use health::DatabaseHealth;
pub use metering::{MeteringReconciliationCounts, MeteringRepository, MeteringWriteOutcome};
pub use migrations::{MIGRATOR, MigrationRunError, run_migrations};
pub use postgres_control_plane::*;
pub use storage_error::{StorageError, StorageFailureKind};
