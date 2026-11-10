//! Credentials read from the environment and never printed.

use std::fmt;

/// A credential value. `Debug` and `Display` print `[redacted]`; use [`Secret::expose`]
/// only at the point the value goes onto the wire.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretError {
    #[error("{0} is not set; the host must pass it in the connector's environment")]
    Missing(String),
    #[error("{0} is set but empty")]
    Empty(String),
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Reads a required secret from the environment, trimming surrounding whitespace.
    pub fn from_env(name: &str) -> Result<Self, SecretError> {
        match std::env::var(name) {
            Ok(v) if v.trim().is_empty() => Err(SecretError::Empty(name.into())),
            Ok(v) => Ok(Self(v.trim().to_owned())),
            Err(_) => Err(SecretError::Missing(name.into())),
        }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Replaces every occurrence of each secret in `text` with `[redacted]`.
/// Secrets shorter than 4 characters are ignored so that scrubbing can't mangle ordinary text.
pub fn scrub(text: &str, secrets: &[&Secret]) -> String {
    let mut out = text.to_owned();
    for s in secrets {
        if s.0.len() >= 4 {
            out = out.replace(&s.0, "[redacted]");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_prints() {
        let s = Secret::new("AIzaSuperSecret");
        assert_eq!(format!("{s}"), "[redacted]");
        assert_eq!(format!("{s:?}"), "Secret([redacted])");
    }

    #[test]
    fn scrubs_all_occurrences() {
        let s = Secret::new("AIzaSuperSecret");
        let t = scrub("key=AIzaSuperSecret and again AIzaSuperSecret", &[&s]);
        assert_eq!(t, "key=[redacted] and again [redacted]");
    }

    #[test]
    fn short_secrets_are_not_scrubbed() {
        let s = Secret::new("a");
        assert_eq!(scrub("banana", &[&s]), "banana");
    }
}
