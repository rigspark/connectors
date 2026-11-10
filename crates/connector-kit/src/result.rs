//! Turning connector results into MCP tool results.

use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;

/// A successful tool result carrying `value` as pretty JSON text.
pub fn json_result<T: Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(s) => CallToolResult::success(vec![ContentBlock::text(s)]),
        Err(e) => tool_error(format!("could not encode the result: {e}")),
    }
}

/// A tool-level error (`isError: true`): the model sees the message and can retry or explain.
/// Use this for upstream API failures and bad arguments. The message must already be scrubbed.
pub fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}
