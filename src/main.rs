use anyhow::Result;
use clap::Parser;
use sret::{config::Config, server::Server};
use std::path::PathBuf;
use tracing::{Level, error, info};

#[derive(Parser, Debug)]
#[command(name = "sret")]
#[command(about = "A Rust reverse proxy with load balancing")]
struct Args {
    #[arg(short, long, default_value = "config.yaml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let args = Args::parse();

    info!("Starting sret reverse proxy");
    info!("Loading configuration from: {}", args.config.display());

    let config = Config::from_file(&args.config)?;

    let mut tasks = Vec::new();

    for server_config in &config.servers {
        let server = Server::new(&config, server_config);
        let task = tokio::spawn(async move {
            if let Err(e) = server.start().await {
                error!("Server {} failed: {}", server.id, e);
            }
        });
        tasks.push(task);
    }

    if tasks.is_empty() {
        error!("No servers configured!");
        return Ok(());
    }

    info!("Started {} server(s)", tasks.len());

    for task in tasks {
        let _ = task.await;
    }

    Ok(())
}
