//! CLI-level acceptance tests for the query layer (M1.b.3, spec §7.1).
//! `carto where` lands first; `carto deps`/`carto map` are added to this
//! same file as each is implemented, against the same
//! `fixtures/rust-crate` index used by `cli_extraction.rs` — it already
//! exercises every §5.3 resolution tier, which is exactly what makes it a
//! good traversal target too.

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/rust-crate")
}

fn golden_path(command: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../fixtures/rust-crate.{command}.golden.json"))
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-query-test-{tag}-{}-{:?}",
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

fn index(out: &Path) {
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(fixture_path())
        .arg("--out")
        .arg(out)
        .assert()
        .success();
}

/// Compares `actual` (already pretty-printed) against a checked-in golden
/// file, same `CARTO_UPDATE_GOLDEN=1` mechanism as `cli.rs`'s
/// `index_matches_golden_graph_json`. No `carto_version` normalization
/// needed here — query results don't carry it.
fn assert_matches_golden(command: &str, actual_pretty: &str) {
    if std::env::var_os("CARTO_UPDATE_GOLDEN").is_some() {
        std::fs::write(golden_path(command), format!("{actual_pretty}\n")).unwrap();
        return;
    }
    let golden = std::fs::read_to_string(golden_path(command)).unwrap_or_else(|e| {
        panic!(
            "failed to read golden file {}: {e}\nrun with CARTO_UPDATE_GOLDEN=1 to create it",
            golden_path(command).display()
        )
    });
    assert_eq!(
        actual_pretty.trim_end(),
        golden.trim_end(),
        "`carto {command}` output for fixtures/rust-crate no longer matches \
         fixtures/rust-crate.{command}.golden.json.\n\
         If this change is expected, regenerate with:\n  \
         CARTO_UPDATE_GOLDEN=1 cargo test -p carto-cli --test cli_query"
    );
}

#[test]
fn where_matches_golden_json() {
    let out = TempDir::new("where-golden");
    index(out.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("summary")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let pretty = serde_json::to_string_pretty(&value).unwrap();
    assert_matches_golden("where", &pretty);
}

#[test]
fn where_exact_and_substring_behave_differently_at_the_cli_boundary() {
    let out = TempDir::new("where-exact");
    index(out.path());

    // Substring, case-insensitive default: "order" matches multiple
    // symbols (parse_order, audit_order, log_order, Order itself isn't a
    // function/method match target since it's a struct — still named
    // "Order", which contains "order" case-insensitively).
    let substring = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("order")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--json")
        .output()
        .unwrap();
    let substring_value: serde_json::Value = serde_json::from_slice(&substring.stdout).unwrap();
    let substring_count = substring_value["matches"].as_array().unwrap().len();
    assert!(
        substring_count > 1,
        "expected multiple substring matches for `order`, got {substring_count}"
    );

    // Exact, case-sensitive: no symbol is literally named "order".
    let exact = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("order")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--exact")
        .arg("--json")
        .output()
        .unwrap();
    let exact_value: serde_json::Value = serde_json::from_slice(&exact.stdout).unwrap();
    assert_eq!(exact_value["matches"].as_array().unwrap().len(), 0);
}

/// A `carto where` run against a directory that was never indexed exits 1
/// (spec §9.2: user error) with a message naming the fix.
#[test]
fn where_against_unindexed_dir_exits_1_and_names_index() {
    let out = TempDir::new("where-unindexed");
    // Deliberately skip calling index().

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("anything")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("carto index"),
        "expected stderr to name `carto index` as the fix, got: {stderr}"
    );
}
