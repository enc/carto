//! End-to-end tests for `carto serve` (spec §7.3's MCP stdio server) via
//! the real compiled binary — same `assert_cmd` pattern `cli.rs` uses for
//! `index`. Unlike `carto-mcp`'s own unit tests (which drive `serve()`
//! directly over in-memory buffers), these exercise the actual process
//! boundary: real stdin/stdout, real process exit code.

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mixed")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-mcp-test-{tag}-{}-{:?}",
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

fn lines_of(stdout: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8(stdout.to_vec())
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON: {l:?}: {e}")))
        .collect()
}

/// A newline-delimited JSON-RPC batch: `initialize`, the
/// `notifications/initialized` notification, then `tools/list` — the
/// standard MCP client handshake sequence.
fn handshake_batch() -> String {
    [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    ]
    .join("\n")
        + "\n"
}

/// `carto serve` behind stdin closing (EOF) exits 0 — a clean shutdown,
/// not an error condition.
#[test]
fn serve_exits_zero_on_stdin_eof() {
    Command::cargo_bin("carto")
        .unwrap()
        .arg("serve")
        .write_stdin("")
        .assert()
        .success();
}

/// The full initialize -> initialized -> tools/list handshake, over the
/// real process: two responses (the notification gets none), correct
/// protocol version, and the exact M1 tool set (spec §7.1's tools this
/// milestone actually implements, no M2+ names).
#[test]
fn serve_handshake_and_tools_list_round_trip_through_the_real_binary() {
    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("serve")
        .write_stdin(handshake_batch())
        .output()
        .unwrap();
    assert!(output.status.success());

    let msgs = lines_of(&output.stdout);
    assert_eq!(msgs.len(), 2, "the notification must get no response");
    assert_eq!(msgs[0]["result"]["protocolVersion"], "2024-11-05");

    let names: Vec<String> = msgs[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        vec![
            "index",
            "where",
            "deps",
            "map",
            "contract",
            "orphans",
            "selfcheck"
        ]
    );
}

/// `tools/call` for `where`, against a real index built by a real `carto
/// index` invocation moments earlier — the same round trip an MCP client
/// would make: index once, then query. Confirms the fencing/truncation
/// contract survives the real stdin/stdout boundary, not just the
/// in-process `serve()` unit tests.
#[test]
fn serve_where_tool_call_answers_against_a_real_index() {
    let out = TempDir::new("where");
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();

    let out_path = out.path().to_string_lossy().replace('\\', "\\\\");
    let repo_path = fixture_path().to_string_lossy().replace('\\', "\\\\");
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"where","arguments":{{"repo_path":"{repo_path}","out":"{out_path}","needle":""}}}}}}"#
    );

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("serve")
        .write_stdin(format!("{request}\n"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let msgs = lines_of(&output.stdout);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["result"]["isError"], false);
    assert!(msgs[0]["result"]["structuredContent"]["matches"].is_array());
}

/// `tools/call` against a repo that was never indexed: a *tool-level*
/// failure (`isError: true` inside a normal JSON-RPC success response),
/// not a process crash or a JSON-RPC protocol error — the request itself
/// was well-formed.
#[test]
fn serve_tool_call_against_an_unindexed_repo_is_a_tool_error_not_a_crash() {
    let repo_path = fixture_path().to_string_lossy().replace('\\', "\\\\");
    let never_indexed = std::env::temp_dir().join(format!(
        "carto-mcp-never-indexed-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let out_path = never_indexed.to_string_lossy().replace('\\', "\\\\");
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"where","arguments":{{"repo_path":"{repo_path}","out":"{out_path}","needle":"x"}}}}}}"#
    );

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("serve")
        .write_stdin(format!("{request}\n"))
        .output()
        .unwrap();
    assert!(output.status.success(), "the process itself must not fail");

    let msgs = lines_of(&output.stdout);
    assert_eq!(msgs[0]["result"]["isError"], true);
    assert!(
        msgs[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("carto index")
    );
}

/// A malformed line followed by a well-formed one: the malformed line
/// gets a JSON-RPC parse-error response, the loop survives, and the good
/// request that follows still gets answered.
#[test]
fn serve_survives_a_malformed_line_through_the_real_binary() {
    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("serve")
        .write_stdin("not json at all\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
        .output()
        .unwrap();
    assert!(output.status.success());

    let msgs = lines_of(&output.stdout);
    assert_eq!(msgs.len(), 2);
    assert!(msgs[0]["error"]["code"].as_i64().is_some());
    assert_eq!(msgs[1]["result"], serde_json::json!({}));
}

/// ADR-0044 over MCP: `deps` on a Terraform `variable` carries the same
/// caveat in the text block and the structured field, round-tripped
/// through the real process boundary.
#[test]
fn serve_deps_on_a_terraform_variable_carries_the_external_inputs_caveat() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/tf-modules");
    let out = TempDir::new("tfvars-deps");
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(&repo)
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();

    let out_path = out.path().to_string_lossy().replace('\\', "\\\\");
    let repo_path = repo.to_string_lossy().replace('\\', "\\\\");
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"deps","arguments":{{"repo_path":"{repo_path}","out":"{out_path}","target":"var.region","dir":"in","subpath":"infra/envs/prod"}}}}}}"#
    );
    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("serve")
        .write_stdin(format!("{request}\n"))
        .output()
        .unwrap();
    assert!(output.status.success());

    let msgs = lines_of(&output.stdout);
    assert_eq!(msgs[0]["result"]["isError"], false, "{msgs:?}");
    let text = msgs[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("not evidence it is unset or unused"),
        "{text}"
    );
    assert!(text.contains("infra/envs/prod/prod.tfvars"), "{text}");
    assert_eq!(
        msgs[0]["result"]["structuredContent"]["root_may_be_set_externally"]["tfvars_files"][1],
        "infra/envs/prod/prod.tfvars"
    );
    assert!(!text.contains("CANARY"));
}
