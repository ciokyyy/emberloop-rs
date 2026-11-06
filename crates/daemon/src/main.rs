use clap::Parser;
use emberloop_daemon::{Cli, Command, Config};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
    let cli = Cli::parse();
    let command = cli.command.clone();
    let config = Config::from_cli(&cli);
    match command {
        Some(Command::Mcp { root, spill }) => emberloop_daemon::run_mcp(root, spill).await,
        None => emberloop_daemon::Daemon::open(config).await?.serve().await,
    }
}
