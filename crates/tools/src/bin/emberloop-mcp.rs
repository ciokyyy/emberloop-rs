use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tools::{McpServer, ToolConfig, Toolbox};

#[derive(Parser)]
#[command(
    name = "emberloop-mcp",
    about = "MCP server for emberloop project tools"
)]
struct Args {
    /// Project root: every tool path resolves inside it.
    #[arg(long)]
    root: PathBuf,
    /// Spill directory, usually <data root>/sessions/<id>/spill.
    #[arg(long)]
    spill: PathBuf,
    /// Rendered tool output above this many bytes spills to a ctx handle.
    #[arg(long, default_value_t = 10 * 1024)]
    overflow_threshold: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let config = ToolConfig {
        overflow_threshold: args.overflow_threshold,
        ..ToolConfig::new(args.root, args.spill)
    };
    let toolbox = Arc::new(Toolbox::new(config)?);
    Arc::new(McpServer::new(toolbox)).serve_stdio().await?;
    Ok(())
}
