//! Rausu — High-performance LLM API Gateway
//!
//! Entry point: parses CLI arguments, loads configuration,
//! initialises logging, and starts the HTTP server.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::info;

mod auth;
mod config;
mod init;
mod providers;
mod schema;
mod server;
mod transform;

use crate::config::{paths::resolve_config_path, AppConfig};
use crate::server::Server;

/// Rausu LLM API Gateway
#[derive(Parser, Debug)]
#[command(name = "rausu", version, about = "High-performance LLM API Gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to the YAML configuration file.
    ///
    /// When omitted, Rausu searches for a config file in well-known locations
    /// (see `RAUSU_CONFIG` env var and the auto-discovery order documented in
    /// `src/config/paths.rs`).  If no file is found a template is written to
    /// `${XDG_CONFIG_HOME}/rausu/config.yaml` and the process exits so you can
    /// edit it first.
    #[arg(short, long)]
    config: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Generate a template configuration file and exit.
    ///
    /// Writes a commented YAML template to the default location
    /// (`${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml`) unless `--path` is
    /// given.  The file is not overwritten unless `--force` is also passed.
    Init {
        /// Target path for the config file.
        ///
        /// Defaults to `${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml`.
        #[arg(long)]
        path: Option<String>,

        /// Overwrite the file if it already exists.
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { path, force }) => init::run_init(path.as_deref(), force),
        None => run_serve(cli.config.as_deref()).await,
    }
}

async fn run_serve(cli_config: Option<&str>) -> Result<()> {
    let config_path = match resolve_config_path(cli_config) {
        Some(p) => p,
        None => {
            let default_path = config::paths::default_config_path();
            init::write_template(&default_path, false).with_context(|| {
                format!(
                    "Failed to write template config to {} — check directory permissions",
                    default_path.display()
                )
            })?;
            eprintln!();
            eprintln!("No configuration file found.  A template has been created at:");
            eprintln!("  {}", default_path.display());
            eprintln!();
            eprintln!("Edit the file with your credentials, then run `rausu` again.");
            eprintln!("To see all init options, run `rausu init --help`.");
            eprintln!();
            anyhow::bail!("Configuration required — edit the template and restart");
        }
    };

    let app_config = AppConfig::load(
        config_path
            .to_str()
            .context("Config path contains non-UTF-8 characters")?,
    )?;

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
        config = %config_path.display(),
        "Rausu starting"
    );

    let server = Server::new(app_config)?;
    server.run().await
}
