//! `connector.toml`: what a connector is, what it needs from the host, and what each tool may do.
//!
//! The manifest is the contract the crew reads before launching a connector: which env vars
//! (and which of them are secrets) to pass, and which tools are `read` versus `write` so the
//! approval policy can ask before any write. The server's own MCP annotations are hints a
//! server could get wrong; the manifest is checked against them in each connector's tests.

use std::collections::BTreeSet;

use rmcp::model::Tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub connector: Connector,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connector {
    pub name: String,
    pub version: String,
    pub description: String,
    /// Only `stdio` today.
    pub transport: String,
    /// Binary name the host runs.
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvVar {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    /// The host keeps this in its credential store and scrubs it from output.
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Reads only; safe to allow by default.
    Read,
    /// Changes something upstream; the crew asks before running it.
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    pub name: String,
    pub access: Access,
    /// Upstream cost per call in the API's own units (e.g. YouTube quota units), if any.
    #[serde(default)]
    pub cost: Option<u32>,
    #[serde(default)]
    pub notes: Option<String>,
    /// Uses an undocumented upstream endpoint that may break or be disallowed; the host
    /// can choose not to enable it.
    #[serde(default)]
    pub unofficial: bool,
    /// Extra credential the tool needs beyond the connector's required env, e.g. `oauth`.
    #[serde(default)]
    pub requires: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("connector.toml is not valid: {0}")]
    Parse(String),
    #[error("connector.toml is inconsistent: {0}")]
    Invalid(String),
}

impl Manifest {
    pub fn parse(src: &str) -> Result<Self, ManifestError> {
        let m: Self = toml::from_str(src).map_err(|e| ManifestError::Parse(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<(), ManifestError> {
        let bad = |s: String| Err(ManifestError::Invalid(s));
        if self.connector.transport != "stdio" {
            return bad(format!(
                "transport `{}` is not supported",
                self.connector.transport
            ));
        }
        let mut seen = BTreeSet::new();
        for t in &self.tools {
            if !seen.insert(&t.name) {
                return bad(format!("tool `{}` is listed twice", t.name));
            }
        }
        for e in &self.env {
            if e.name.is_empty()
                || !e
                    .name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                return bad(format!("env var `{}` must be UPPER_SNAKE_CASE", e.name));
            }
        }
        Ok(())
    }

    pub fn tool(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Checks the manifest against the tools the server actually lists: the same names, and
    /// `read` tools (and only those) annotated `readOnlyHint: true`.
    pub fn check_tools(&self, tools: &[Tool]) -> Result<(), ManifestError> {
        let declared: BTreeSet<&str> = self.tools.iter().map(|t| t.name.as_str()).collect();
        let served: BTreeSet<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        let mut problems = Vec::new();
        for n in served.difference(&declared) {
            problems.push(format!("server has `{n}` but connector.toml doesn't"));
        }
        for n in declared.difference(&served) {
            problems.push(format!("connector.toml has `{n}` but the server doesn't"));
        }
        for t in tools {
            if let Some(spec) = self.tool(t.name.as_ref()) {
                let ro = t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true);
                match (spec.access, ro) {
                    (Access::Read, false) => problems.push(format!(
                        "`{}` is `read` but not annotated read-only",
                        t.name
                    )),
                    (Access::Write, true) => {
                        problems.push(format!("`{}` is `write` but annotated read-only", t.name))
                    }
                    _ => {}
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(ManifestError::Invalid(problems.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
[connector]
name = "x"
version = "0.1.0"
description = "d"
transport = "stdio"
command = "x"

[[env]]
name = "X_KEY"
description = "k"
required = true
secret = true

[[tools]]
name = "a"
access = "read"
cost = 1
"#;

    #[test]
    fn parses() {
        let m = Manifest::parse(OK).unwrap();
        assert_eq!(m.tool("a").unwrap().access, Access::Read);
        assert!(m.env[0].secret);
    }

    #[test]
    fn rejects_unknown_fields_and_duplicates() {
        assert!(Manifest::parse(&OK.replace("cost = 1", "cost = 1\nsneaky = true")).is_err());
        let dup = format!("{OK}\n[[tools]]\nname = \"a\"\naccess = \"write\"\n");
        assert!(matches!(
            Manifest::parse(&dup),
            Err(ManifestError::Invalid(_))
        ));
        assert!(Manifest::parse(&OK.replace("X_KEY", "x-key")).is_err());
    }
}
