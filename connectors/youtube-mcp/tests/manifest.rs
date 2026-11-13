//! connector.toml must describe exactly the tools the server lists, with matching access.

use connector_kit::{Access, Manifest};
use youtube_mcp::{Config, MANIFEST, YouTubeServer};

fn server() -> YouTubeServer {
    YouTubeServer::new(Config::for_tests("http://127.0.0.1:9")).unwrap()
}

#[test]
fn manifest_matches_server() {
    let m = Manifest::parse(MANIFEST).expect("connector.toml parses");
    m.check_tools(&server().tools())
        .expect("manifest and server agree");

    assert_eq!(m.connector.name, "youtube-mcp");
    assert_eq!(
        m.connector.version,
        env!("CARGO_PKG_VERSION"),
        "bump connector.toml with Cargo.toml"
    );
    let key = m
        .env
        .iter()
        .find(|e| e.name == youtube_mcp::KEY_ENV)
        .unwrap();
    assert!(key.required && key.secret);
    // every write needs OAuth; nothing unofficial writes
    for t in &m.tools {
        if t.access == Access::Write {
            assert_eq!(t.requires.as_deref(), Some("oauth"), "{}", t.name);
            assert!(!t.unofficial, "{}", t.name);
        }
    }
    for e in m.env.iter().filter(|e| {
        e.name.contains("SECRET") || e.name.contains("TOKEN") || e.name.ends_with("_KEY")
    }) {
        assert!(e.secret, "{} must be marked secret", e.name);
    }
    assert_eq!(m.tools.len(), 32);
    assert_eq!(
        m.tools.iter().filter(|t| t.access == Access::Write).count(),
        5
    );
}

#[test]
fn every_tool_has_a_description_and_object_schema() {
    for t in server().tools() {
        assert!(
            t.description.as_deref().is_some_and(|d| d.len() > 20),
            "{} needs a description",
            t.name
        );
        assert_eq!(
            t.input_schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "{}",
            t.name
        );
    }
}
