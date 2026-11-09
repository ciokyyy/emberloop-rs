use clap::Parser;
use emberloop_daemon::{Cli, Command, Config};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stderr: the `mcp` subcommand speaks JSON-RPC on stdout.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
    let cli = Cli::parse();
    let command = cli.command.clone();
    let config = Config::from_cli(&cli);
    match command {
        Some(Command::Mcp {
            root,
            spill,
            global_skills,
        }) => {
            emberloop_daemon::run_mcp(root, spill, config.config_root.clone(), global_skills).await
        }
        Some(Command::Update {
            manifest,
            session,
            check,
            rollback,
            file,
            major,
            next_version,
        }) => {
            emberloop_daemon::run_update(emberloop_daemon::UpdateRequest {
                manifest_url: manifest
                    .unwrap_or_else(emberloop_daemon::update::default_manifest_url),
                session,
                check,
                rollback,
                data_root: config.data_root.clone(),
                file,
                major,
                next_version,
            })
            .await
        }
        None => emberloop_daemon::Daemon::open(config).await?.serve().await,
    }
}
