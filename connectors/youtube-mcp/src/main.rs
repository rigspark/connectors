use connector_kit::{Secret, serve};
use youtube_mcp::config::{CLIENT_ID_ENV, CLIENT_SECRET_ENV, Endpoints, REFRESH_ENV};
use youtube_mcp::{Config, MANIFEST, YouTubeServer};

const HELP: &str = "youtube-mcp: RigSpark MCP connector for YouTube (stdio).

USAGE:
    youtube-mcp                        serve MCP over stdin/stdout
    youtube-mcp auth [--write]         sign in to your channel and print a refresh token
    youtube-mcp --manifest             print connector.toml
    youtube-mcp --version

ENV (server):
    YOUTUBE_API_KEY               required. YouTube Data API v3 key (secret).
    YOUTUBE_OAUTH_CLIENT_ID       optional. OAuth client for your-channel tools.
    YOUTUBE_OAUTH_CLIENT_SECRET   optional (secret).
    YOUTUBE_OAUTH_REFRESH_TOKEN   optional (secret), from `youtube-mcp auth`.
    YOUTUBE_MCP_UPLOAD_DIR        optional. The only folder uploads/thumbnails may read from.
    YOUTUBE_MCP_DB                optional. History database path, or `off`.
                                  Default ~/.rigspark/youtube-mcp/history.sqlite
    YOUTUBE_MCP_UNOFFICIAL        optional. `off` disables autocomplete and transcripts.
    YOUTUBE_API_BASE              optional. Point every upstream at one mock (tests).
    RUST_LOG                      optional. Log level on stderr, default info.

ENV (auth):
    YOUTUBE_OAUTH_CLIENT_ID and YOUTUBE_OAUTH_CLIENT_SECRET from a Google Cloud
    'Desktop app' OAuth client. --write also grants editing, comments and uploads.";

async fn auth(write: bool) -> std::process::ExitCode {
    let id = std::env::var(CLIENT_ID_ENV).unwrap_or_default();
    let secret = match Secret::from_env(CLIENT_SECRET_ENV) {
        Ok(s) if !id.trim().is_empty() => s,
        _ => {
            eprintln!(
                "set {CLIENT_ID_ENV} and {CLIENT_SECRET_ENV} (a Google Cloud OAuth client of type 'Desktop app') first"
            );
            return 1.into();
        }
    };
    let token_url = std::env::var(youtube_mcp::BASE_ENV)
        .map(|b| Endpoints::under(&b).token)
        .unwrap_or_else(|_| Endpoints::production().token);
    match youtube_mcp::oauth::run_auth(id.trim(), &secret, write, &token_url).await {
        Ok(refresh) => {
            eprintln!(
                "\nSigned in. Store this in your credential store and pass it to the connector as {REFRESH_ENV}:\n"
            );
            println!("{REFRESH_ENV}={}", refresh.expose());
            eprintln!(
                "\n(On macOS: security add-generic-password -a youtube-mcp -s {REFRESH_ENV} -w '<token>')"
            );
            0.into()
        }
        Err(e) => {
            eprintln!("youtube-mcp auth: {e}");
            1.into()
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
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
        Some("auth") => return auth(args.iter().any(|a| a == "--write")).await,
        Some(other) => {
            eprintln!("unknown argument `{other}`\n\n{HELP}");
            return 2.into();
        }
        None => {}
    }

    serve::init_tracing();
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("youtube-mcp: {e}");
            return 1.into();
        }
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        oauth = cfg.oauth.is_some(),
        unofficial = cfg.unofficial,
        history = cfg.db.is_some(),
        uploads = cfg.upload_dir.is_some(),
        "youtube-mcp ready on stdio"
    );
    let server = match YouTubeServer::new(cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("youtube-mcp: {e}");
            return 1.into();
        }
    };
    match serve::stdio(server).await {
        Ok(()) => 0.into(),
        Err(e) => {
            tracing::error!(error = %e, "server stopped");
            1.into()
        }
    }
}
