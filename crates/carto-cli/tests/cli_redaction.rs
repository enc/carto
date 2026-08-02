//! CLI-level acceptance test for the redaction pass (spec §7.5,
//! ADR-0017), against `fixtures/secrets-corpus` (see its own README
//! for the exact scenario matrix). Confirms end-to-end behavior at the
//! `carto index` boundary: no raw secret substring reaches `graph.json`,
//! every category fires, and every "clean" symbol is untouched.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/secrets-corpus")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-redaction-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn index(out: &Path) -> (Value, Value) {
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(fixture_path())
        .arg("--out")
        .arg(out)
        .assert()
        .success();
    let graph: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("graph.json")).unwrap()).unwrap();
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    (graph, manifest)
}

fn symbol_signature<'a>(graph: &'a Value, name: &str) -> &'a str {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "symbol" && n["name"] == name)
        .unwrap_or_else(|| panic!("no symbol node named `{name}`"))["signature"]
        .as_str()
        .unwrap_or_else(|| panic!("`{name}` has no signature"))
}

/// The exact fake secret substrings this fixture plants — none of these
/// may appear anywhere in `graph.json`.
const PLANTED_SECRETS: &[&str] = &[
    "AKIAIOSFODNN7EXAMPLE",
    "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    "ghp_1234567890abcdef1234567890abcdef1234",
    "-----BEGIN RSA PRIVATE KEY-----",
    "xJ4kLpQ9rT2vN8mF6wZ1yB3cH5dS7aE",
    "hunter2example",
    "xoxb-1234567890-abcdefghijklmnop",
    "glpat-1234567890abcdefWXYZ",
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dQw4w9WgXcQ-rDpZ5RwFj0",
];

#[test]
fn no_planted_secret_reaches_graph_json() {
    let out = TempDir::new("no-leak");
    let (_graph, _manifest) = index(out.path());
    let raw = std::fs::read_to_string(out.path().join("graph.json")).unwrap();
    for secret in PLANTED_SECRETS {
        assert!(
            !raw.contains(secret),
            "graph.json must not contain the raw secret `{secret}`"
        );
    }
}

#[test]
fn every_category_fires_exactly_once() {
    let out = TempDir::new("categories");
    let (_graph, manifest) = index(out.path());

    let by_category = manifest["redaction"]["by_category"].as_object().unwrap();
    let expected = [
        "aws_access_key",
        "secret_key_near_keyword",
        "private_key_pem",
        "github_token",
        "gitlab_token",
        "slack_token",
        "jwt",
        "connection_string",
        "high_entropy",
    ];
    for category in expected {
        assert_eq!(
            by_category.get(category).and_then(Value::as_u64),
            Some(1),
            "category `{category}` must fire exactly once"
        );
    }
    assert_eq!(by_category.len(), expected.len(), "no extra categories");
}

#[test]
fn redacted_signatures_carry_the_replacement_marker() {
    let out = TempDir::new("marker");
    let (graph, _manifest) = index(out.path());

    for name in [
        "AWS_ACCESS_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "GITHUB_TOKEN",
        "TLS_KEY_HEADER",
        "SESSION_TOKEN",
        "dbConnection",
        "slackWebhookToken",
        "gitlabToken",
        "jwtExample",
    ] {
        let sig = symbol_signature(&graph, name);
        assert!(
            sig.contains("«redacted:"),
            "`{name}`'s signature must carry a redaction marker, got: {sig}"
        );
    }
}

#[test]
fn clean_symbols_are_byte_identical_to_source() {
    let out = TempDir::new("clean");
    let (graph, _manifest) = index(out.path());

    assert_eq!(
        symbol_signature(&graph, "GIT_COMMIT_REF"),
        r#"pub const GIT_COMMIT_REF: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";"#
    );
    assert_eq!(
        symbol_signature(&graph, "SERVER_PORT"),
        "pub const SERVER_PORT: u16 = 8080;"
    );
    assert_eq!(
        symbol_signature(&graph, "serviceName"),
        r#"serviceName = "orders-api""#
    );
}

/// Spec §9.5 gate 3 (INV-7), same pattern as every other extractor's
/// determinism test — redaction's sha256-based replacement tokens are
/// deterministic per matched substring, so this must hold too.
#[test]
fn redaction_output_is_byte_identical_across_two_runs() {
    let out_a = TempDir::new("det-a");
    let out_b = TempDir::new("det-b");
    index(out_a.path());
    index(out_b.path());

    let a = std::fs::read(out_a.path().join("graph.json")).unwrap();
    let b = std::fs::read(out_b.path().join("graph.json")).unwrap();
    assert_eq!(a, b);
}
