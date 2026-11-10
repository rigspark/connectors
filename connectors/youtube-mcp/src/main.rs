use connector_kit::{Secret, serve};
use youtube_mcp::{BASE_ENV, KEY_ENV, MANIFEST, YouTube, YouTubeServer, api::DEFAULT_BASE};

const HELP: &str = "youtube-mcp: RigSpark MCP connector for YouTube (stdio).

USAGE:
    youtube-mcp              serve MCP over stdin/stdout
    youtube-mcp --manifest   print connector.toml
    youtube-mcp --version

ENV:
    YOUTUBE_API_KEY    required. A YouTube Data API v3 key (secret).
    YOUTUBE_API_BASE   optional. Override the API root (tests, proxies).
    RUST_LOG           optional. Log level on stderr, default info.";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--manifest") => {
            print!("{MANIFEST}");
            return 0.into();
        }
        Some("--version" | "-V") => {
            println!("youtube-mcp {}", env!("CARGO_PKG_VERSION"));
            return 0.into();
        }
        Some("--help" | "-h") => {
            println!("{HELP}");
            return 0.into();
        }
        Some(other) => {
            eprintln!("unknown argument `{other}`\n\n{HELP}");
            return 2.into();
        }
        None => {}
    }

    serve::init_tracing();
    let key = match Secret::from_env(KEY_ENV) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("youtube-mcp: {e}");
            return 1.into();
        }
    };
    let base = std::env::var(BASE_ENV).unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    let yt = match YouTube::new(key, &base) {
        Ok(y) => y,
        Err(e) => {
            eprintln!("youtube-mcp: {e}");
            return 1.into();
        }
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "youtube-mcp ready on stdio"
    );
    match serve::stdio(YouTubeServer::new(yt)).await {
        Ok(()) => 0.into(),
        Err(e) => {
            tracing::error!(error = %e, "server stopped");
            1.into()
        }
    }
}
