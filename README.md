# rigspark/connectors

RigSpark's MCP connectors, all in Rust. Each connector is a small **stdio MCP server**
that the RigSpark crew (or any MCP host: Claude Desktop, Claude Code, Cursor…) launches as
a child process with the credentials it needs in its environment.

Private. Internal to RigSpark.

| Connector | What it does | Access | Needs |
| --- | --- | --- | --- |
| [`youtube-mcp`](connectors/youtube-mcp) | vidIQ-style YouTube research (keywords, outliers, hot videos, competitors, comment insights, title/SEO scores, earnings, transcripts), your own analytics and a one-call channel health check, video edits/uploads, local history. 32 tools, 6 prompts | read + 5 OAuth writes | `YOUTUBE_API_KEY` (+ OAuth for your channel) |

## Layout

```
crates/connector-kit/      shared rules every connector follows (secrets, HTTP, results, manifest)
connectors/<name>/         one crate per connector, binary named <name>
  connector.toml           the manifest: env it needs, every tool and whether it reads or writes
  src/                     api.rs (upstream client) · tools.rs (MCP tools) · main.rs
  tests/                   manifest.rs (manifest == server) · stdio.rs (real MCP client vs mock API)
scripts/new-connector.sh   scaffold the next one
docs/                      conventions and how to add a connector
```

## The rules (enforced by `connector-kit` and the tests)

1. **Secrets come from env only**, never from args or files. They're `Secret` values that
   can't be printed, go in headers rather than URLs, and are scrubbed from every error. This
   matches the crew's credential gateway: the host keeps the secret and only the connector's
   environment gets it.
2. **stdout carries the protocol.** Logs go to stderr (`RUST_LOG`).
3. **Every tool is declared in `connector.toml`** with `access = "read"` or `"write"`. The
   crew's approval policy reads this file and asks before any write. A test fails if the manifest
   and the server's tool list or `readOnlyHint`s disagree.
4. **Model-friendly results.** The output is compact JSON with long text trimmed and ids
   plus URLs on everything. Upstream failures come back as tool errors (`isError: true`) with a
   message the model can act on ("quota used up, try later"), not as protocol errors.
5. **Validate before calling upstream.** Bad ids, dates and enums are rejected locally, so
   no quota is spent on them.
6. **Third-party text is untrusted.** Comments, descriptions and so on are data. Mark them as
   such in tool descriptions and manifest notes so the crew quotes them as untrusted input.

## Build and test

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release -p youtube-mcp        # target/release/youtube-mcp
```

Toolchain pinned to Rust 1.98.1 (`rust-toolchain.toml`), the same as `rigspark` and `rigspark-crew`.

## Using a connector

Any MCP host config works. For example (Claude Desktop / Claude Code `mcpServers`):

```json
{
  "mcpServers": {
    "youtube": {
      "command": "/path/to/youtube-mcp",
      "env": { "YOUTUBE_API_KEY": "…" }
    }
  }
}
```

`youtube-mcp --manifest` prints the manifest, so a host can learn what to pass without
reading the repo.

## Adding a connector

```bash
scripts/new-connector.sh whoop-mcp WHOOP_ACCESS_TOKEN
```

Then follow [docs/adding-a-connector.md](docs/adding-a-connector.md).

## Releases

Tag `<connector>-v<version>` (e.g. `youtube-mcp-v0.1.0`). CI builds macOS (arm64, x86_64)
and Linux (x86_64) binaries and attaches them, with checksums, to a GitHub release.
