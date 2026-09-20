//! Process-owned storage selection shared by serve and administrator commands.

use ai_gateway::{
    persistence::{
        AuthRepository, ControlPlaneRepository, DatabaseHealth, RequestLogRepository,
        run_migrations,
    },
    runtime_config::DatabaseConfig,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
#[cfg(feature = "sqlite-backend")]
use std::sync::Arc;
use std::{error::Error, time::Duration};

pub enum Database {
    Postgres {
        control: PgPool,
        logs: PgPool,
    },
    #[cfg(feature = "sqlite-backend")]
    Sqlite(Arc<ai_gateway::persistence::sqlite::SqliteDatabase>),
}

impl Database {
    pub async fn open(
        config: &DatabaseConfig,
        log_connections: Option<u32>,
    ) -> Result<Self, Box<dyn Error>> {
        if let Some(path) = config.sqlite_path()? {
            #[cfg(feature = "sqlite-backend")]
            {
                let database = ai_gateway::persistence::sqlite::SqliteDatabase::open_with_limits(
                    &path,
                    config.max_connections,
                    Duration::from_secs(config.connect_timeout_seconds),
                )
                .await?;
                if let Err(error) = database.install_schema().await {
                    database.close().await;
                    return Err(error.into());
                }
                return Ok(Self::Sqlite(Arc::new(database)));
            }
            #[cfg(not(feature = "sqlite-backend"))]
            {
                let _ = path;
                unreachable!("configuration rejects SQLite without its feature");
            }
        }
        let options = config.connect_options()?;
        let control = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(Duration::from_secs(config.connect_timeout_seconds))
            .connect_with(options.clone().application_name("ai-gateway-control-plane"))
            .await?;
        let setup = async {
            run_migrations(&control).await?;
            let logs = if let Some(capacity) = log_connections {
                PgPoolOptions::new()
                    .max_connections(capacity)
                    .acquire_timeout(Duration::from_secs(config.connect_timeout_seconds))
                    .connect_with(options.application_name("ai-gateway-request-log"))
                    .await?
            } else {
                control.clone()
            };
            Ok::<_, Box<dyn Error>>(logs)
        }
        .await;
        match setup {
            Ok(logs) => Ok(Self::Postgres { control, logs }),
            Err(error) => {
                control.close().await;
                Err(error)
            }
        }
    }
    pub fn control(&self) -> ControlPlaneRepository {
        match self {
            Self::Postgres { control, .. } => ControlPlaneRepository::new(control.clone()),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(database) => ControlPlaneRepository::from_sqlite(Arc::clone(database)),
        }
    }
    pub fn auth(&self) -> AuthRepository {
        match self {
            Self::Postgres { control, .. } => AuthRepository::new(control.clone()),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(database) => AuthRepository::from_sqlite(Arc::clone(database)),
        }
    }
    pub fn logs(&self) -> RequestLogRepository {
        match self {
            Self::Postgres { logs, .. } => RequestLogRepository::new(logs.clone()),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(database) => RequestLogRepository::from_sqlite(Arc::clone(database)),
        }
    }
    pub fn health(&self) -> DatabaseHealth {
        match self {
            Self::Postgres { control, .. } => DatabaseHealth::from(control.clone()),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(database) => DatabaseHealth::from_sqlite(Arc::clone(database)),
        }
    }
    pub async fn close(&self) {
        match self {
            Self::Postgres { control, logs } => {
                logs.close().await;
                control.close().await;
            }
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(database) => database.close().await,
        }
    }
}
