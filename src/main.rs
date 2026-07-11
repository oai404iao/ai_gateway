use std::{error::Error, path::PathBuf, sync::Arc};

use ai_gateway::{application::ProxyService, http, observability, runtime_config::AppConfig};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));
    let config = AppConfig::load(&config_path)?.compile()?;

    observability::init(&config.observability.filter);

    let address = format!("{}:{}", config.server.host, config.server.port);
    let proxy = ProxyService::new(
        Arc::new(config.runtime),
        config.server.max_request_body_bytes,
        &config.upstream,
    )?;
    let listener = TcpListener::bind(&address).await?;
    tracing::info!(%address, "AI gateway listening");

    axum::serve(listener, http::router(proxy)).await?;
    Ok(())
}
