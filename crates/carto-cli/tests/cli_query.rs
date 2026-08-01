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
fn deps_matches_golden_json() {
    let out = TempDir::new("deps-golden");
    index(out.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("handle")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--depth")
        .arg("2")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let pretty = serde_json::to_string_pretty(&value).unwrap();
    assert_matches_golden("deps", &pretty);
}

/// `--depth` above `consts::MAX_DEPS_DEPTH` is a user error at the CLI
/// boundary too, not just in `query::deps::run`'s own unit test.
#[test]
fn deps_target_out_of_range_depth_exits_1() {
    let out = TempDir::new("deps-depth");
    index(out.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("handle")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--depth")
        .arg("99")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("exceeds the cap"), "got: {stderr}");
}

#[test]
fn deps_dir_in_finds_the_containing_file() {
    let out = TempDir::new("deps-dir-in");
    index(out.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("handle")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--dir")
        .arg("in")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hops = value["hops"].as_array().unwrap();
    assert_eq!(hops.len(), 1);
    let edges = hops[0]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["kind"], "contains");
    assert_eq!(edges[0]["confidence"], "certain");
    assert_eq!(edges[0]["node"]["label"], "src/handlers.rs");
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

/// `map`'s output is prose (spec §7.1's "layered overview"), which is
/// fragile to golden-file against — this asserts the budget contract
/// (§7.1: "hard-capped at budget") at two different budgets instead of
/// snapshotting exact text, same choice `cli.rs`'s pattern reserves
/// golden files for structurally stable output.
#[test]
fn map_respects_budget_at_multiple_sizes() {
    let out = TempDir::new("map-budget");
    index(out.path());

    for budget in [5u32, 50] {
        let output = Command::cargo_bin("carto")
            .unwrap()
            .arg("map")
            .arg(fixture_path())
            .arg("--out")
            .arg(out.path())
            .arg("--budget")
            .arg(budget.to_string())
            .output()
            .unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        // The truncation notice (when present) isn't part of the
        // budgeted overview itself, so only count lines up to it.
        let overview_lines = stdout
            .lines()
            .take_while(|l| !l.starts_with("... truncated"))
            .count();
        assert!(
            overview_lines <= budget as usize,
            "budget {budget}: got {overview_lines} lines"
        );
    }
}

/// `deps`'s root `root_unresolved_calls` end-to-end, human output: real
/// pain point from field feedback was `--dir out` returning empty with
/// no way to tell "calls nothing" from "everything unresolved" — this
/// asserts the human-readable line actually appears, not just the
/// `--json` field (already covered by `deps_matches_golden_json`).
#[test]
fn deps_human_output_shows_root_unresolved_calls() {
    let out = TempDir::new("deps-unresolved-human");
    index(out.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("handle")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("unresolved call") && stdout.contains("unknown_external_call"),
        "got: {stdout}"
    );
}

/// `where --subpath` end-to-end (spec §7.1): `"order"` (substring)
/// matches `log_order` (src/handlers.rs), `parse_order`/`audit_order`/
/// `Order` (src/orders.rs) unscoped; `--subpath src/orders.rs` must
/// exclude `log_order` specifically, not just shrink the count.
#[test]
fn where_subpath_restricts_matches_to_the_given_file() {
    let out = TempDir::new("where-subpath");
    index(out.path());

    let unscoped = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("order")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--json")
        .output()
        .unwrap();
    let unscoped_value: serde_json::Value = serde_json::from_slice(&unscoped.stdout).unwrap();
    let unscoped_names: Vec<String> = unscoped_value["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        unscoped_names.contains(&"log_order".to_string()),
        "{unscoped_names:?}"
    );

    let scoped = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("order")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--subpath")
        .arg("src/orders.rs")
        .arg("--json")
        .output()
        .unwrap();
    assert!(scoped.status.success());
    let scoped_value: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    let scoped_names: Vec<String> = scoped_value["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !scoped_names.contains(&"log_order".to_string()),
        "{scoped_names:?}"
    );
    assert!(scoped_names.contains(&"parse_order".to_string()));
    assert!(scoped_names.contains(&"audit_order".to_string()));
}

/// `deps --subpath` end-to-end: proves the BFS still traverses through
/// an out-of-scope hop (`log_order`, src/handlers.rs) to reach a
/// deeper in-scope one (`validate`, src/orders.rs), not just that
/// out-of-scope rows are hidden.
#[test]
fn deps_subpath_hides_an_out_of_scope_hop_but_keeps_traversing() {
    let out = TempDir::new("deps-subpath");
    index(out.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("handle")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--dir")
        .arg("out")
        .arg("--depth")
        .arg("2")
        .arg("--subpath")
        .arg("src/orders.rs")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hops = value["hops"].as_array().unwrap();

    let hop1_labels: Vec<&str> = hops[0]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["node"]["label"].as_str().unwrap())
        .collect();
    assert!(
        !hop1_labels.contains(&"log_order"),
        "log_order (src/handlers.rs) must be hidden: {hop1_labels:?}"
    );
    assert!(hop1_labels.contains(&"parse_order"));

    // validate is reached via parse_order at depth 2 — only findable if
    // the BFS kept expanding past handle's other, non-hidden neighbors
    // regardless of scoping.
    let all_labels: Vec<&str> = hops
        .iter()
        .flat_map(|h| h["edges"].as_array().unwrap())
        .map(|e| e["node"]["label"].as_str().unwrap())
        .collect();
    assert!(all_labels.contains(&"validate"), "{all_labels:?}");
}

/// `map --subpath` end-to-end: restricts the file count to just the
/// named file, proving the CLI flag actually reaches `MapQuery` (unlike
/// the positional PATH argument, which — per the real-world feedback
/// that motivated this flag — silently doesn't).
#[test]
fn map_subpath_restricts_file_counts() {
    let out = TempDir::new("map-subpath");
    index(out.path());

    let unscoped = Command::cargo_bin("carto")
        .unwrap()
        .arg("map")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--json")
        .output()
        .unwrap();
    let unscoped_value: serde_json::Value = serde_json::from_slice(&unscoped.stdout).unwrap();
    let unscoped_files = unscoped_value["counts"]["files"].as_u64().unwrap();

    let scoped = Command::cargo_bin("carto")
        .unwrap()
        .arg("map")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--subpath")
        .arg("src/handlers.rs")
        .arg("--json")
        .output()
        .unwrap();
    assert!(scoped.status.success());
    let scoped_value: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    assert_eq!(scoped_value["counts"]["files"].as_u64().unwrap(), 1);
    assert!(unscoped_files > 1);
}

#[test]
fn map_json_matches_golden_json() {
    let out = TempDir::new("map-golden");
    index(out.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .arg("map")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let pretty = serde_json::to_string_pretty(&value).unwrap();
    assert_matches_golden("map", &pretty);
}
