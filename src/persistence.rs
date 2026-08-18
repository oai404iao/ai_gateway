//! Database-neutral persistence boundary and backend implementations.
//!
//! Concrete backend modules are deliberately not part of the public API:
//!
//! ```compile_fail
//! use ai_gateway::persistence::postgres::ControlPlaneRepository;
//! ```

mod auth;
mod codex;
mod control_plane;
mod database;
mod error;
mod postgres;
mod records;
mod request_log;
#[cfg(feature = "sqlite-backend")]
mod sqlite;
mod sqlx_adapter;

pub use auth::*;
pub use codex::{
    CodexCredentialBatchInput, CodexCredentialBatchOperation, CodexCredentialBatchTarget,
    CodexCredentialCreate, CodexCredentialExportBundle, CodexCredentialExportInput,
    CodexCredentialExportItem, CodexCredentialExportProxy, CodexCredentialImportInput,
    CodexCredentialRecord, CodexCredentialUpdateInput, CodexCredentialView, CodexOauthFlowRecord,
    CodexOauthStartInput, CodexQuotaResetOutcome, CodexQuotaUpdate, CodexQuotaWindowHistory,
    CodexQuotaWindowPeriodView, CodexTokenRefreshUpdate, SelfCodexQuotaCredentialView,
    SelfCodexQuotaWindowHistory, SelfCodexQuotaWindowPeriodView,
};
pub use control_plane::{
    ApiKeyCreate, ApiKeyPolicyInput, ApiKeyUpdate, ChannelBatchChanges, ChannelBatchUpdateInput,
    ChannelBatchUpdateTarget, ChannelCreateInput, ChannelGroupInput, ChannelInput,
    ChannelRecoverInput, ConfigTemplateCreateInput, ConfigTemplateInput, ConsoleApiKey,
    ConsoleAuditLog, ControlPlaneApiKey, ControlPlaneApiKeyPolicy, ControlPlaneChannel,
    ControlPlaneChannelDetail, ControlPlaneChannelGroup, ControlPlaneConfigTemplate,
    ControlPlaneConfigTemplateDetail, ControlPlaneLists, ControlPlaneMcpServer, ControlPlaneModel,
    ControlPlaneModelRule, ControlPlaneMutation, ControlPlaneProxy, ControlPlaneUser,
    ControlPlaneUserGroup, McpServerCreateInput, McpServerInput, ModelInput, ModelRuleInput,
    ModelRuleRoutingStatus, MutationResult, ProxyCreateInput, ProxyInput, SelfApiKeyChannelOption,
    SelfApiKeyCreate, SelfApiKeyGroupOption, SelfApiKeyOptions, SelfApiKeyUpdate, SyncedModelInput,
    UserBalanceBatchChange, UserBatchChanges, UserBatchUpdateInput, UserBatchUpdateTarget,
    UserGroupInput, UserInput, UserUpdateInput,
};
pub use database::{
    DatabaseBackend, DatabaseConnectOptions, DatabasePool, MIGRATOR, POSTGRES_MIGRATOR,
    RepositoryTransaction, TransactionIntent, run_migrations,
};
pub use error::{RepositoryError, RepositoryErrorSource};
pub use postgres::{AuthRepository, ControlPlaneRepository, RequestLogRepository};
pub use records::*;
pub use request_log::{
    ChannelGroupStatusBucket, ChannelGroupStatusGroup, ChannelGroupStatusGroupModel,
    ChannelGroupStatusModelMetric, ChannelGroupStatusReport, ChannelGroupStatusWindow,
    ConsoleRequestLog, CostStatisticsBucket, CostStatisticsBucketModel, CostStatisticsChannel,
    CostStatisticsFilter, CostStatisticsModel, CostStatisticsReport, CostStatisticsSummary,
    PersonalUsageDay, PersonalUsageReport, RequestLogBatchInsertOutcome,
    RequestLogBatchInsertResult, RequestLogFilter, RequestLogInsertOutcome,
    RequestLogSettlementOutcome, SpendLeaderboardEntry, SpendLeaderboardFilter,
    SpendLeaderboardPeriod, SpendLeaderboardRefresh, SpendLeaderboardReport, StatisticsGranularity,
};
#[allow(unused_imports)]
pub(crate) use request_log::{
    RequestLogIngestBacklog, RequestLogIngestRecord, RequestLogPoolStatus,
    RequestLogSettlementBacklog,
};
#[cfg(feature = "sqlite-backend")]
pub use sqlite::{
    SQLITE_MIGRATOR, SqliteAuthRepository, SqliteDecimal, SqliteRuntimeConfigRepository,
    SqliteStringList, SqliteUuidList,
};
