//! Explicit backend dispatch for the Console and sharing read surfaces:
//! request-log views, channel-group status, personal usage, cost statistics,
//! spend-leaderboard snapshots, and the sharing recovery read.

use chrono::NaiveDate;
use sqlx::PgPool;
use uuid::Uuid;

use super::RepositoryError;
use super::postgres_control_plane::{
    ChannelGroupStatusReport, ChannelGroupStatusWindow, ConsoleRequestLog, CostStatisticsFilter,
    CostStatisticsReport, PersonalUsageReport, PostgresMeteringQueries, PostgresRequestLogQueries,
    RequestLogFilter, SpendLeaderboardFilter, SpendLeaderboardRefresh, SpendLeaderboardReport,
};

#[cfg(feature = "sqlite-backend")]
use std::sync::Arc;

#[cfg(feature = "sqlite-backend")]
use super::sqlite::{
    SqliteDatabase, SqliteMeteringQueries, SqliteRequestLogQueries as SqliteLogQueries,
};

#[derive(Clone)]
enum LogQueriesBackend {
    Postgres(PostgresRequestLogQueries),
    #[cfg(feature = "sqlite-backend")]
    Sqlite {
        queries: SqliteLogQueries,
        database: Arc<SqliteDatabase>,
    },
}

/// Bounded request-log reads for the Console, scoped or global.
#[derive(Clone)]
pub struct RequestLogQueries {
    backend: LogQueriesBackend,
}

impl RequestLogQueries {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            backend: LogQueriesBackend::Postgres(PostgresRequestLogQueries::new(pool)),
        }
    }

    /// Builds a development repository over a file-backed SQLite database.
    ///
    /// The server composition root never selects this constructor; it exists for
    /// backend contract tests and future SQLite enablement in S6.
    #[cfg(feature = "sqlite-backend")]
    #[must_use]
    pub fn from_sqlite(database: Arc<SqliteDatabase>) -> Self {
        Self {
            backend: LogQueriesBackend::Sqlite {
                queries: SqliteLogQueries::new(Arc::clone(&database)),
                database,
            },
        }
    }

    pub(super) fn from_postgres(queries: PostgresRequestLogQueries) -> Self {
        Self {
            backend: LogQueriesBackend::Postgres(queries),
        }
    }

    /// Financial query surface over the same backend and database handle.
    #[must_use]
    pub fn metering(&self) -> MeteringQueries {
        match &self.backend {
            LogQueriesBackend::Postgres(queries) => {
                MeteringQueries::from_postgres(queries.metering())
            }
            #[cfg(feature = "sqlite-backend")]
            LogQueriesBackend::Sqlite { database, .. } => {
                MeteringQueries::from_sqlite(Arc::clone(database))
            }
        }
    }

    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        filter: RequestLogFilter,
    ) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
        match &self.backend {
            LogQueriesBackend::Postgres(queries) => queries.list_for_user(user_id, filter).await,
            #[cfg(feature = "sqlite-backend")]
            LogQueriesBackend::Sqlite { queries, .. } => {
                queries.list_for_user(user_id, filter).await
            }
        }
    }

    pub async fn get_for_user(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
        match &self.backend {
            LogQueriesBackend::Postgres(queries) => queries.get_for_user(user_id, id).await,
            #[cfg(feature = "sqlite-backend")]
            LogQueriesBackend::Sqlite { queries, .. } => queries.get_for_user(user_id, id).await,
        }
    }

    pub async fn list_all(
        &self,
        filter: RequestLogFilter,
    ) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
        match &self.backend {
            LogQueriesBackend::Postgres(queries) => queries.list_all(filter).await,
            #[cfg(feature = "sqlite-backend")]
            LogQueriesBackend::Sqlite { queries, .. } => queries.list_all(filter).await,
        }
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
        match &self.backend {
            LogQueriesBackend::Postgres(queries) => queries.get(id).await,
            #[cfg(feature = "sqlite-backend")]
            LogQueriesBackend::Sqlite { queries, .. } => queries.get(id).await,
        }
    }

    pub async fn channel_group_status(
        &self,
        window: ChannelGroupStatusWindow,
    ) -> Result<ChannelGroupStatusReport, RepositoryError> {
        match &self.backend {
            LogQueriesBackend::Postgres(queries) => queries.channel_group_status(window).await,
            #[cfg(feature = "sqlite-backend")]
            LogQueriesBackend::Sqlite { queries, .. } => queries.channel_group_status(window).await,
        }
    }
}

#[derive(Clone)]
enum MeteringQueriesBackend {
    Postgres(PostgresMeteringQueries),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(SqliteMeteringQueries),
}

/// Financial aggregate reads over immutable facts and snapshot tables.
#[derive(Clone)]
pub struct MeteringQueries {
    backend: MeteringQueriesBackend,
}

impl MeteringQueries {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            backend: MeteringQueriesBackend::Postgres(PostgresMeteringQueries::new(pool)),
        }
    }

    /// Builds a development repository over a file-backed SQLite database.
    ///
    /// The server composition root never selects this constructor; it exists for
    /// backend contract tests and future SQLite enablement in S6.
    #[cfg(feature = "sqlite-backend")]
    #[must_use]
    pub fn from_sqlite(database: Arc<SqliteDatabase>) -> Self {
        Self {
            backend: MeteringQueriesBackend::Sqlite(SqliteMeteringQueries::new(database)),
        }
    }

    fn from_postgres(queries: PostgresMeteringQueries) -> Self {
        Self {
            backend: MeteringQueriesBackend::Postgres(queries),
        }
    }

    pub async fn personal_usage(
        &self,
        user_id: Uuid,
        ended_on: NaiveDate,
    ) -> Result<PersonalUsageReport, RepositoryError> {
        match &self.backend {
            MeteringQueriesBackend::Postgres(queries) => {
                queries.personal_usage(user_id, ended_on).await
            }
            #[cfg(feature = "sqlite-backend")]
            MeteringQueriesBackend::Sqlite(queries) => {
                queries.personal_usage(user_id, ended_on).await
            }
        }
    }

    pub async fn cost_statistics(
        &self,
        filter: CostStatisticsFilter,
    ) -> Result<CostStatisticsReport, RepositoryError> {
        match &self.backend {
            MeteringQueriesBackend::Postgres(queries) => queries.cost_statistics(filter).await,
            #[cfg(feature = "sqlite-backend")]
            MeteringQueriesBackend::Sqlite(queries) => queries.cost_statistics(filter).await,
        }
    }

    pub async fn refresh_spend_leaderboard_snapshots(
        &self,
    ) -> Result<SpendLeaderboardRefresh, RepositoryError> {
        match &self.backend {
            MeteringQueriesBackend::Postgres(queries) => {
                queries.refresh_spend_leaderboard_snapshots().await
            }
            #[cfg(feature = "sqlite-backend")]
            MeteringQueriesBackend::Sqlite(queries) => {
                queries.refresh_spend_leaderboard_snapshots().await
            }
        }
    }

    pub async fn spend_leaderboard(
        &self,
        filter: SpendLeaderboardFilter,
    ) -> Result<SpendLeaderboardReport, RepositoryError> {
        match &self.backend {
            MeteringQueriesBackend::Postgres(queries) => queries.spend_leaderboard(filter).await,
            #[cfg(feature = "sqlite-backend")]
            MeteringQueriesBackend::Sqlite(queries) => queries.spend_leaderboard(filter).await,
        }
    }

    /// Recovery read for in-flight Codex sharing reservations.
    pub async fn sharing_completed_costs(
        &self,
        ids: &[Uuid],
    ) -> Result<Vec<(Uuid, rust_decimal::Decimal)>, RepositoryError> {
        match &self.backend {
            MeteringQueriesBackend::Postgres(queries) => queries.sharing_completed_costs(ids).await,
            #[cfg(feature = "sqlite-backend")]
            MeteringQueriesBackend::Sqlite(queries) => queries.sharing_completed_costs(ids).await,
        }
    }
}
