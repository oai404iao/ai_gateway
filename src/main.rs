use std::{error::Error, path::PathBuf, sync::Arc};

use ai_gateway::{
    application::ProxyService,
    http, observability,
    persistence::{ControlPlaneRepository, MIGRATOR},
    runtime_config::{AppConfig, RuntimeConfig, compile_control_plane},
    workers::ControlPlaneReloader,
};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));
    let config = AppConfig::load(&config_path)?.validate()?;

    observability::init(&config.observability.filter);

    let pool = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .acquire_timeout(std::time::Duration::from_secs(
            config.database.connect_timeout_seconds,
        ))
        .connect(&config.database.url)
        .await?;
    MIGRATOR.run(&pool).await?;
    let repository = ControlPlaneRepository::new(pool);
    let initial = compile_control_plane(repository.load().await?)?;
    let runtime = Arc::new(RuntimeConfig::new(initial));

    let address = format!("{}:{}", config.server.host, config.server.port);
    let proxy = ProxyService::new(
        Arc::clone(&runtime),
        config.server.max_request_body_bytes,
        &config.upstream,
    )?;
    ControlPlaneReloader::new(repository, runtime).spawn(std::time::Duration::from_secs(
        config.runtime_config.reload_interval_seconds,
    ));
    let listener = TcpListener::bind(&address).await?;
    tracing::info!(%address, "AI gateway listening");

    axum::serve(listener, http::router(proxy)).await?;
    Ok(())
}
