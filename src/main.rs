use anyhow::Result;
use clap::Parser;
use sret::{config::Config, proxy::ReverseProxy};
use std::path::PathBuf;
use tracing::{Level, info};

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

    let proxy = ReverseProxy::new(config).await?;
    proxy.start().await?;

    Ok(())
}
