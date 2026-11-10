//! Process plumbing shared by every connector binary.

use rmcp::{ServerHandler, ServiceExt};
use tracing_subscriber::EnvFilter;

/// Logs to **stderr** (stdout carries the MCP protocol). Level from `RUST_LOG`, default `info`.
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

/// Serves `handler` over stdio until the client disconnects or the process gets Ctrl-C.
pub async fn stdio<H: ServerHandler>(handler: H) -> Result<(), Box<dyn std::error::Error>> {
    let service = handler.serve(rmcp::transport::stdio()).await?;
    tokio::select! {
        r = service.waiting() => { r?; }
        _ = tokio::signal::ctrl_c() => { tracing::info!("interrupted, shutting down"); }
    }
    Ok(())
}
