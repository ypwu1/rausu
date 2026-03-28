//! Rausu — High-performance LLM API Gateway
//!
//! Entry point: parses CLI arguments, loads configuration,
//! initialises logging, and starts the HTTP server.

use anyhow::Result;
use clap::Parser;
use tracing::info;

mod config;
mod providers;
mod schema;
mod server;

use crate::config::AppConfig;
use crate::server::Server;

/// Rausu LLM API Gateway
#[derive(Parser, Debug)]
#[command(name = "rausu", version, about = "High-performance LLM API Gateway")]
struct Cli {
    /// Path to the YAML configuration file
    #[arg(short, long, default_value = "config.yaml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let app_config = AppConfig::load(&cli.config)?;

    // Initialise logging based on config
    let log_level = app_config.logging.level.as_deref().unwrap_or("info");
    let use_json = app_config.logging.format.as_deref().unwrap_or("json") == "json";

    if use_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
            )
            .init();
    } else {
        tracing_subscriber::fmt()
            .pretty()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
            )
            .init();
    }

    info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %cli.config,
        "Rausu starting"
    );

    let server = Server::new(app_config)?;
    server.run().await
}
