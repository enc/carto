//! CLI-level acceptance tests for the Rust extractor (M1.b.2a, spec
//! §5.2/§5.3), against `fixtures/rust-crate` (see its README for the
//! exact scenario matrix). Semantic assertions on parsed JSON rather
//! than a byte-for-byte golden file — a golden JSON for a symbol-rich
//! fixture is fragile to hand-review; `fixtures/mixed`'s golden-file
//! pattern (see `cli.rs`) stays reserved for walk-only coverage.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/rust-crate")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-extraction-test-{tag}-{}-{:?}",
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

fn index(out: &Path) -> Value {
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(fixture_path())
        .arg("--out")
        .arg(out)
        .assert()
        .success();
    let graph_json = std::fs::read_to_string(out.join("graph.json")).unwrap();
    serde_json::from_str(&graph_json).unwrap()
}

fn symbols(graph: &Value) -> Vec<&Value> {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "symbol")
        .collect()
}

fn symbol_named<'a>(graph: &'a Value, name: &str) -> &'a Value {
    symbols(graph)
        .into_iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no symbol node named `{name}` in graph.json"))
}

fn id_to_name(graph: &Value, id: &str) -> String {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .and_then(|n| n["name"].as_str().or_else(|| n["path"].as_str()))
        .unwrap_or("<unknown>")
        .to_string()
}

fn calls_edge<'a>(graph: &'a Value, from_name: &str, to_name: &str) -> &'a Value {
    graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["kind"] == "calls"
                && id_to_name(graph, e["from"].as_str().unwrap()) == from_name
                && id_to_name(graph, e["to"].as_str().unwrap()) == to_name
        })
        .unwrap_or_else(|| panic!("no calls edge {from_name} -> {to_name} in graph.json"))
}

fn references_edge<'a>(graph: &'a Value, from_name: &str, to_name: &str) -> &'a Value {
    graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["kind"] == "references"
                && id_to_name(graph, e["from"].as_str().unwrap()) == from_name
                && id_to_name(graph, e["to"].as_str().unwrap()) == to_name
        })
        .unwrap_or_else(|| panic!("no references edge {from_name} -> {to_name} in graph.json"))
}

/// ADR-0029: `parse_order`'s own `-> Order` return type is a type
/// position nothing captured before this — verifies the CLI/JSON
/// surface end to end, not just the extractor's own unit tests.
#[test]
fn type_position_return_type_produces_a_references_edge() {
    let out = TempDir::new("references");
    let graph = index(out.path());

    let edge = references_edge(&graph, "parse_order", "Order");
    assert_eq!(edge["confidence"], "inferred");
    assert_eq!(
        edge["evidence"].as_array().unwrap(),
        &[Value::String("type-reference:same-file".into())]
    );
}

#[test]
fn extracts_every_expected_symbol_with_correct_sym_kind() {
    let out = TempDir::new("symbols");
    let graph = index(out.path());

    let expected: &[(&str, &str)] = &[
        ("Order", "struct"),
        ("summary", "method"),
        ("parse_order", "function"),
        ("validate", "function"),
        ("audit_order", "function"),
        ("handle", "function"),
        ("log_order", "function"),
    ];
    for (name, sym_kind) in expected {
        let sym = symbol_named(&graph, name);
        assert_eq!(sym["sym_kind"], *sym_kind, "wrong sym_kind for `{name}`");
    }
    assert_eq!(
        symbols(&graph).len(),
        expected.len(),
        "unexpected extra/missing symbols"
    );
}

#[test]
fn impl_block_method_has_qualified_stable_id_distinct_from_a_same_named_function_would_have() {
    let out = TempDir::new("qualified-id");
    let graph = index(out.path());
    // The qualified-name-in-ID recipe (spec §4.3) means `Order::summary`
    // and a hypothetical bare `summary` function would never collide —
    // asserted indirectly here by confirming the method symbol exists
    // and is uniquely identified (`symbol_named` already panics on zero
    // matches; this confirms it isn't a duplicate either).
    let matches: Vec<&Value> = symbols(&graph)
        .into_iter()
        .filter(|s| s["name"] == "summary")
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "impl-block method must be extracted exactly once"
    );
}

#[test]
fn calls_resolve_via_every_tier_with_inferred_confidence() {
    let out = TempDir::new("calls");
    let graph = index(out.path());

    let cases: &[(&str, &str, &str)] = &[
        ("parse_order", "validate", "same-file"),
        ("handle", "log_order", "same-file"),
        ("handle", "parse_order", "imported"),
        ("handle", "audit_order", "same-package"),
        ("handle", "summary", "same-package"),
    ];
    for (from, to, evidence) in cases {
        let edge = calls_edge(&graph, from, to);
        assert_eq!(edge["confidence"], "inferred", "{from} -> {to}");
        assert_eq!(
            edge["evidence"].as_array().unwrap(),
            &[Value::String(evidence.to_string())],
            "{from} -> {to}"
        );
    }
}

#[test]
fn unresolved_calls_are_recorded_honestly_not_guessed() {
    let out = TempDir::new("unresolved");
    let graph = index(out.path());

    let handle = symbol_named(&graph, "handle");
    let unresolved: Vec<&str> = handle["unresolved_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(unresolved, vec!["unknown_external_call"]);

    let validate = symbol_named(&graph, "validate");
    let unresolved: Vec<&str> = validate["unresolved_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        unresolved,
        vec!["is_empty"],
        "stdlib method calls have no type info to resolve against — honestly unresolved, not a defect"
    );
}

#[test]
fn mod_declarations_produce_certain_imports_edges_to_sibling_files() {
    let out = TempDir::new("imports");
    let graph = index(out.path());

    let imports: Vec<&Value> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "imports")
        .collect();
    assert_eq!(imports.len(), 2);
    for edge in imports {
        assert_eq!(edge["confidence"], "certain");
        assert_eq!(
            edge["evidence"].as_array().unwrap(),
            &[Value::String("mod-declaration".to_string())]
        );
    }
}

#[test]
fn every_symbol_has_a_contains_edge_from_its_file() {
    let out = TempDir::new("contains");
    let graph = index(out.path());

    let contains_count = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "contains")
        .count();
    assert_eq!(contains_count, symbols(&graph).len());
}

/// Spec §9.5 gate 3 (INV-7), same pattern as `cli.rs`'s determinism test
/// but for a fixture whose graph actually has symbols/edges in it, not
/// just `File` nodes.
#[test]
fn extraction_output_is_byte_identical_across_two_runs() {
    let out_a = TempDir::new("det-a");
    let out_b = TempDir::new("det-b");

    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(fixture_path())
        .arg("--out")
        .arg(out_a.path())
        .assert()
        .success();
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(fixture_path())
        .arg("--out")
        .arg(out_b.path())
        .assert()
        .success();

    let a = std::fs::read(out_a.path().join("graph.json")).unwrap();
    let b = std::fs::read(out_b.path().join("graph.json")).unwrap();
    assert_eq!(a, b);
}
