//! connector.toml must describe exactly the tools the server lists, with matching access.

use connector_kit::{Access, Manifest, Secret};
use youtube_mcp::{MANIFEST, YouTube, YouTubeServer};

#[test]
fn manifest_matches_server() {
    let m = Manifest::parse(MANIFEST).expect("connector.toml parses");
    let server =
        YouTubeServer::new(YouTube::new(Secret::new("unused-key"), "http://127.0.0.1:9").unwrap());
    m.check_tools(&server.tools())
        .expect("manifest and server agree");

    assert_eq!(m.connector.name, "youtube-mcp");
    assert_eq!(
        m.connector.version,
        env!("CARGO_PKG_VERSION"),
        "bump connector.toml with Cargo.toml"
    );
    assert!(
        m.tools.iter().all(|t| t.access == Access::Read),
        "youtube-mcp is read-only"
    );
    let key = m
        .env
        .iter()
        .find(|e| e.name == youtube_mcp::KEY_ENV)
        .unwrap();
    assert!(key.required && key.secret);
}

#[test]
fn every_tool_has_a_description_and_object_schema() {
    let server =
        YouTubeServer::new(YouTube::new(Secret::new("unused-key"), "http://127.0.0.1:9").unwrap());
    for t in server.tools() {
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
