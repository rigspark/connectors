//! connector.toml must describe exactly the tools the server lists, with matching access.

use std::collections::BTreeSet;

use connector_kit::{Access, Manifest};
use whoop_mcp::MANIFEST;
use whoop_mcp::catalog::{PrivacyMode, tools};

fn names(mode: PrivacyMode) -> BTreeSet<String> {
    tools(mode)
        .iter()
        .map(|t| t["name"].as_str().expect("tool name").to_owned())
        .collect()
}

#[test]
fn manifest_matches_server() {
    let m = Manifest::parse(MANIFEST).expect("connector.toml parses");
    assert_eq!(m.connector.name, "whoop-mcp");
    assert_eq!(
        m.connector.version,
        env!("CARGO_PKG_VERSION"),
        "bump connector.toml with Cargo.toml"
    );

    let declared: BTreeSet<String> = m.tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(
        declared,
        names(PrivacyMode::Standard),
        "manifest and server agree"
    );
    assert!(
        names(PrivacyMode::Aggregate).is_subset(&declared),
        "aggregate mode only serves declared tools"
    );

    assert!(
        m.tools.iter().all(|t| t.access == Access::Read),
        "whoop-mcp is read-only"
    );
    for mode in [PrivacyMode::Standard, PrivacyMode::Aggregate] {
        for t in tools(mode) {
            assert_eq!(
                t["annotations"]["readOnlyHint"].as_bool(),
                Some(true),
                "{} must be annotated read-only",
                t["name"]
            );
        }
    }

    for name in ["WHOOP_CLIENT_ID", "WHOOP_CLIENT_SECRET"] {
        let e = m.env.iter().find(|e| e.name == name).unwrap();
        assert!(e.required && e.secret, "{name}");
    }
}
