//! Shared building blocks for RigSpark MCP connectors.
//!
//! Every connector in this repo is a small stdio MCP server that the RigSpark crew (or any
//! MCP client) launches as a child process. They all follow the same rules, and this crate
//! is where those rules live so each connector doesn't re-invent them:
//!
//! * **Secrets come from the environment only** ([`Secret`]). They are never logged, never
//!   put in URLs when the API allows a header, and scrubbed from anything handed back to the
//!   model ([`scrub`]). This matches the crew's credential gateway: the host holds the
//!   secret and passes it to the connector process in its env.
//! * **stdout is the protocol.** Logs go to stderr ([`serve::init_tracing`]).
//! * **Results are compact JSON for a model** ([`result`]): only the fields it needs, long
//!   text trimmed ([`text::trim`]), upstream failures returned as tool errors (`isError`)
//!   rather than protocol errors so the model can read and react to them.
//! * **Every tool is declared in `connector.toml`** ([`manifest`]) with its access level
//!   (`read` or `write`), so the crew's approval policy can gate writes without trusting
//!   the server's own hints. Each connector has a test that keeps the manifest and the
//!   server's tool list identical.

pub mod http;
pub mod manifest;
pub mod result;
pub mod secret;
pub mod serve;
pub mod text;

pub use http::{HttpClient, HttpError};
pub use manifest::{Access, Manifest};
pub use result::{json_result, tool_error};
pub use secret::{Secret, SecretError, scrub};
