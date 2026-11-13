# Adding a connector

1. **Scaffold.** `scripts/new-connector.sh <name>-mcp <SECRET_ENV>` creates
   `connectors/<name>-mcp` with the standard layout, a placeholder tool, a manifest and both tests.
   The workspace picks it up automatically (`members = ["connectors/*-mcp", "connectors/*/*-mcp"]`).
   To group it by category (like `connectors/fitness/whoop-mcp`), move the folder into
   `connectors/<category>/`; the workspace, CI and release workflows find it there too. Crate
   names must stay unique across categories, and must end in `-mcp`.

2. **Upstream client (`src/api.rs`).**
   - Build on `connector_kit::HttpClient`. Put the credential in a header with
     `with_auth_header`. Only use a query parameter if the API can't take a header.
   - Map upstream errors to an enum whose `Display` tells the model what to do next.
   - Return your own compact `Serialize` structs, not raw API JSON. Trim free text with
     `connector_kit::text::trim` and include a URL for every object.

3. **Tools (`src/tools.rs`).**
   - One `#[tool]` per action, with a description that says what it returns and what it costs.
   - Set `annotations(read_only_hint = true, open_world_hint = true)` on reads. Set
     `read_only_hint = false` and `destructive_hint` honestly on writes.
   - Validate arguments locally and return `tool_error(..)` for bad input before calling upstream.
   - Return `Ok(json_result(..))` or `Ok(tool_error(..))`. Keep `Err(McpError)` for real
     protocol failures.

4. **Manifest (`connector.toml`).** List every env var (mark secrets `secret = true`) and
   every tool with `access`, plus `cost` if the API meters calls. Keep `version` equal to the
   crate version; `tests/manifest.rs` checks it.

5. **Tests.**
   - `tests/manifest.rs`: manifest == server (scaffolded; leave it).
   - `tests/stdio.rs`: launch the binary with a `wiremock` base URL. Cover a happy path for each
     tool, that the secret never appears in URLs or output, the upstream error mapping, and that
     bad arguments never reach upstream (`.expect(0)`).
   - Never hit the real API in CI.

6. **Docs.** Add a `README.md` for the connector and a row in the top-level README table.

7. **Release.** Bump the version in `Cargo.toml` and `connector.toml`, then tag
   `<name>-mcp-v<version>`.

## Write tools

The crew treats `access = "write"` as "ask first", so a write tool must:

- do exactly one upstream change per call, described in the tool's description;
- echo back what changed (ids and URLs) so the approval and audit log entries are meaningful;
- be idempotent where the API allows (`idempotent_hint = true`).

## Manifest flags

- `requires = "oauth"` (or another credential name) on tools that need more than the
  connector's required env. List the tool anyway, and return a clear tool error explaining
  the setup when the credential is missing, so the manifest stays static.
- `unofficial = true` on tools that use undocumented upstream endpoints. Give the host an
  env switch to turn them off, never send the official API credential to them, and only
  follow URLs from their responses when they point back at the provider's own hosts.
