//! Repository operations with explicit backend dispatch; SQLite is not yet selectable in production.

mod auth;
mod backend_auth;
mod backend_codex;
mod backend_control_plane;
mod backend_pipeline;
mod backend_queries;
pub mod capability_cutover;
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
mod upstream_credentials;
pub mod upstream_topology;

use auth::PostgresAuthRepository;
pub use auth::{
    ConsoleProfile, ConsoleSession, ConsoleSessionState, InvitationCreated, InviteUserInput,
    LiveConsoleIdentity, LoginUser, PasswordUser, RegistrationAttempt, RegistrationInvitationCode,
    RegistrationInvitationCodeInput, RegistrationInvitationCodeMutation, SessionRotation,
    SessionUser, TemporaryPasswordCreated,
};
pub use backend_auth::AuthRepository;
pub use backend_codex::{CodexQuotaReset, CodexRefresh, SharingLedgerLease};
pub use backend_control_plane::{ControlPlaneRepository, PreparedControlPlaneChange};
pub use backend_pipeline::{MeteringRepository, RequestLogRepository, SettlementRepository};
pub use backend_queries::{MeteringQueries, RequestLogQueries};
pub use codex::{
    CodexCredentialBatchInput, CodexCredentialBatchOperation, CodexCredentialBatchTarget,
    CodexCredentialCreate, CodexCredentialExportBundle, CodexCredentialExportInput,
    CodexCredentialExportItem, CodexCredentialExportProxy, CodexCredentialImportInput,
    CodexCredentialRecord, CodexCredentialUpdateInput, CodexCredentialView, CodexOauthFlowRecord,
    CodexOauthStartInput, CodexQuotaResetOutcome, CodexQuotaUpdate, CodexQuotaWindowHistory,
    CodexQuotaWindowPeriodView, CodexTokenRefreshUpdate, SelfCodexQuotaCredentialView,
    SelfCodexQuotaWindowHistory, SelfCodexQuotaWindowPeriodView,
};
pub use health::DatabaseHealth;
pub use metering::{MeteringReconciliationCounts, MeteringWriteOutcome};
pub use migrations::{MIGRATOR, MigrationRunError, run_migrations};
pub use postgres_control_plane::*;
pub use storage_error::{StorageError, StorageFailureKind};
pub use upstream_credentials::{
    CredentialIdentity, UpstreamCredentialDetail, UpstreamCredentialInput, UpstreamCredentialView,
};
pub use upstream_topology::ModelRoutingProfileBinding;
#[cfg(feature = "sqlite-backend")]
pub use upstream_topology::sqlite_load;
pub use upstream_topology::{
    ApiKeyCapabilityGrantInput, ApiKeyCapabilityGrantRecord, ApiKeyChannelGrantRecord,
    ApiKeyPolicyCapabilityGrantInput, ApiKeyPolicyCapabilityGrantRecord,
    ApiKeyPolicyChannelGrantRecord, ChannelCapabilityInput, ChannelCapabilityRecord,
    GrantOriginKind, LogicalChannelInput, LogicalChannelRecord, OperationCandidateInput,
    OperationCandidateRecord, OperationRuleInput, OperationRuleRecord, OperationTierInput,
    OperationTierRecord, RoutingGroupInput, RoutingGroupRecord, UpstreamAccessInput,
    UpstreamAccessRecord, UpstreamTopologyRecords, pg_load,
};
