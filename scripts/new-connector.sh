#!/usr/bin/env bash
# Scaffold a new connector: scripts/new-connector.sh <name>-mcp <SECRET_ENV> [API_BASE_URL]
# Creates connectors/<name>-mcp with one placeholder read tool, a manifest and both tests.
set -euo pipefail

name="${1:-}"; secret="${2:-}"; base="${3:-https://api.example.com}"
if [[ ! "$name" =~ ^[a-z][a-z0-9-]*-mcp$ ]]; then
  echo "usage: $0 <name>-mcp <SECRET_ENV> [API_BASE_URL]   (name must be kebab-case ending in -mcp)" >&2; exit 2
fi
if [[ ! "$secret" =~ ^[A-Z][A-Z0-9_]*$ ]]; then
  echo "SECRET_ENV must be UPPER_SNAKE_CASE, e.g. WHOOP_ACCESS_TOKEN" >&2; exit 2
fi
root="$(cd "$(dirname "$0")/.." && pwd)"
dir="$root/connectors/$name"
[[ -e "$dir" ]] && { echo "$dir already exists" >&2; exit 1; }
crate="${name//-/_}"
prefix="${secret%%_*}"
base_env="${prefix}_API_BASE"
mkdir -p "$dir/src" "$dir/tests"

cat > "$dir/Cargo.toml" <<EOF
[package]
name = "$name"
description = "RigSpark MCP connector for TODO."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[[bin]]
name = "$name"
path = "src/main.rs"

[dependencies]
connector-kit.workspace = true
rmcp.workspace = true
schemars.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true

[dev-dependencies]
wiremock.workspace = true
anyhow.workspace = true
rmcp = { workspace = true, features = ["client", "transport-child-process"] }
tokio = { workspace = true, features = ["process"] }

[lints]
workspace = true
EOF

cat > "$dir/connector.toml" <<EOF
[connector]
name = "$name"
version = "0.1.0"
description = "TODO: one line on what this connector reaches."
transport = "stdio"
command = "$name"

[[env]]
name = "$secret"
description = "TODO: where to get it and what scope it needs."
required = true
secret = true

[[env]]
name = "$base_env"
description = "Override the API root. Leave unset in production."
required = false
secret = false

[[tools]]
name = "ping"
access = "read"
EOF

cat > "$dir/src/lib.rs" <<EOF
//! \`$name\`: TODO.

pub mod tools;

pub use tools::Server;

pub const MANIFEST: &str = include_str!("../connector.toml");
pub const KEY_ENV: &str = "$secret";
pub const BASE_ENV: &str = "$base_env";
pub const DEFAULT_BASE: &str = "$base";
EOF

cat > "$dir/src/tools.rs" <<'EOF'
use connector_kit::{HttpClient, json_result, tool_error};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PingArgs {
    /// Path to GET on the API, e.g. /me.
    pub path: String,
}

#[derive(Clone)]
pub struct Server {
    http: HttpClient,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    pub fn new(http: HttpClient) -> Self {
        Self { http, tool_router: Self::tool_router() }
    }

    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    // TODO: replace with real tools. Keep connector.toml in step.
    #[tool(
        name = "ping",
        description = "Placeholder: GETs a path on the API and returns the JSON. Replace me.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn ping(&self, Parameters(a): Parameters<PingArgs>) -> Result<CallToolResult, McpError> {
        if !a.path.starts_with('/') {
            return Ok(tool_error("`path` must start with /"));
        }
        Ok(match self.http.get_json(&a.path, &[]).await {
            Ok(v) => json_result(&v),
            Err(e) => tool_error(e.to_string()),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
            .with_instructions("TODO: tell the model how to use these tools well.")
    }
}
EOF

cat > "$dir/src/main.rs" <<EOF
use connector_kit::{HttpClient, Secret, serve};
use $crate::{BASE_ENV, DEFAULT_BASE, KEY_ENV, MANIFEST, Server};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--manifest") => {
            print!("{MANIFEST}");
            return 0.into();
        }
        Some("--version" | "-V") => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return 0.into();
        }
        Some(other) => {
            eprintln!("unknown argument \`{other}\`; run with no arguments to serve MCP on stdio");
            return 2.into();
        }
        None => {}
    }
    serve::init_tracing();
    let key = match Secret::from_env(KEY_ENV) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{}: {e}", env!("CARGO_PKG_NAME"));
            return 1.into();
        }
    };
    let base = std::env::var(BASE_ENV).unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    let ua = concat!("rigspark-", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
    // TODO: use the header this API expects (e.g. "authorization" with a "Bearer …" secret).
    let http = match HttpClient::new(base, ua) {
        Ok(h) => h.with_auth_header("authorization", Secret::new(format!("Bearer {}", key.expose()))),
        Err(e) => {
            eprintln!("{}: {e}", env!("CARGO_PKG_NAME"));
            return 1.into();
        }
    };
    match serve::stdio(Server::new(http)).await {
        Ok(()) => 0.into(),
        Err(e) => {
            tracing::error!(error = %e, "server stopped");
            1.into()
        }
    }
}
EOF

cat > "$dir/tests/manifest.rs" <<EOF
use connector_kit::{HttpClient, Manifest};
use $crate::{MANIFEST, Server};

#[test]
fn manifest_matches_server() {
    let m = Manifest::parse(MANIFEST).expect("connector.toml parses");
    let server = Server::new(HttpClient::new("http://127.0.0.1:9", "test").unwrap());
    m.check_tools(&server.tools()).expect("manifest and server agree");
    assert_eq!(m.connector.version, env!("CARGO_PKG_VERSION"), "bump connector.toml with Cargo.toml");
}
EOF

cat > "$dir/tests/stdio.rs" <<EOF
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::json;
use wiremock::matchers::{header, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "test-secret-value";

#[tokio::test]
async fn ping_round_trip() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(path("/me"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(env!("CARGO_BIN_EXE_$name")).configure(|cmd| {
            cmd.env("$secret", KEY).env("$base_env", server.uri()).env("RUST_LOG", "warn");
        }),
    )?;
    let client = ().serve(transport).await?;
    let args = json!({"path": "/me"}).as_object().cloned().unwrap();
    let r = client.call_tool(CallToolRequestParams::new("ping").with_arguments(args)).await?;
    assert_ne!(r.is_error, Some(true));
    let text = r.content[0].as_text().unwrap().text.clone();
    assert!(!text.contains(KEY));
    client.cancel().await?;
    Ok(())
}
EOF

cat > "$dir/README.md" <<EOF
# $name

TODO: what this connects to, the tools, setup (\`$secret\`), and what's not done yet.
EOF

(cd "$root" && cargo fmt -p "$name" >/dev/null 2>&1) || true
echo "created connectors/$name"
echo "next: cargo test -p $name, then replace the ping tool (see docs/adding-a-connector.md)"
