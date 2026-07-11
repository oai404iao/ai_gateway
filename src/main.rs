use std::{error::Error, path::PathBuf};

use ai_gateway::{http, observability, runtime_config::AppConfig};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));
    let config = AppConfig::load(&config_path)?;

    observability::init(&config.observability.filter);

    let address = format!("{}:{}", config.server.host, config.server.port);
    let listener = TcpListener::bind(&address).await?;
    tracing::info!(%address, "AI gateway listening");

    axum::serve(listener, http::router()).await?;
    Ok(())
}
