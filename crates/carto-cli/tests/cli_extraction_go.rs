//! CLI-level acceptance tests for the Go extractor (ADR-0015 — the
//! closing slice of M1.b.2b, the last language in spec §5.2's v1 set),
//! against `fixtures/go-svc` (see its own README for the exact scenario
//! matrix). Mirrors `cli_extraction_ts.rs`'s structure/style — semantic
//! assertions on parsed JSON rather than a byte-for-byte golden file.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/go-svc")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-extraction-go-test-{tag}-{}-{:?}",
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

fn node_by_id<'a>(graph: &'a Value, id: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .unwrap()
}

fn id_to_name(graph: &Value, id: &str) -> String {
    let n = node_by_id(graph, id);
    n["name"]
        .as_str()
        .or_else(|| n["path"].as_str())
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

/// ADR-0029: `ParseOrder`'s own `*Order` return type and `Summary`'s
/// own `*Order` receiver are type positions nothing captured before
/// this — the receiver case also proves `parameter_declaration`'s
/// unanchored capture reaches a method's `receiver:` parameter list,
/// not just ordinary parameters.
#[test]
fn type_positions_produce_references_edges() {
    let out = TempDir::new("references");
    let graph = index(out.path());

    for edge in [
        references_edge(&graph, "ParseOrder", "Order"),
        references_edge(&graph, "Summary", "Order"),
    ] {
        assert_eq!(edge["confidence"], "inferred");
        assert_eq!(
            edge["evidence"].as_array().unwrap(),
            &[Value::String("type-reference:same-file".into())]
        );
    }
}

fn imports_edges_from<'a>(graph: &'a Value, from_path: &str) -> Vec<&'a Value> {
    graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["kind"] == "imports" && id_to_name(graph, e["from"].as_str().unwrap()) == from_path
        })
        .collect()
}

#[test]
fn extracts_every_expected_symbol_with_correct_sym_kind() {
    let out = TempDir::new("symbols");
    let graph = index(out.path());

    let expected: &[(&str, &str)] = &[
        ("normalize", "function"),
        ("Auditable", "interface"),
        ("StatusOpen", "var"),
        ("validate", "function"),
        ("ParseOrder", "function"),
        ("AuditOrder", "function"),
        ("Order", "struct"),
        ("DefaultStatus", "const"),
        ("Summary", "method"),
        ("main", "function"),
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
fn method_qualified_name_uses_gos_own_selector_spelling() {
    // Not directly serialized into graph.json (qualified_name only feeds
    // ID construction), but proven indirectly: exactly one `Summary`
    // symbol exists, and it's a method on `Order`'s own file.
    let out = TempDir::new("qualified-name");
    let graph = index(out.path());
    let summary = symbol_named(&graph, "Summary");
    assert_eq!(summary["sym_kind"], "method");
    assert_eq!(
        id_to_name(&graph, summary["file"].as_str().unwrap()),
        "internal/orders/order.go"
    );
}

#[test]
fn calls_resolve_via_every_tier_with_inferred_confidence() {
    let out = TempDir::new("calls");
    let graph = index(out.path());

    let cases: &[(&str, &str, &str)] = &[
        ("ParseOrder", "validate", "same-file"),
        ("ParseOrder", "normalize", "same-directory"),
        ("ParseOrder", "AuditOrder", "same-package"),
        ("main", "ParseOrder", "same-package"),
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

    let main = symbol_named(&graph, "main");
    let unresolved: Vec<&str> = main["unresolved_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    // `Println` (stdlib, no symbol carto sees) and `unknownExternalCall`
    // (no candidate anywhere) — same honesty principle as every other
    // fixture's stdlib-call case.
    assert_eq!(unresolved, vec!["Println", "unknownExternalCall"]);
}

#[test]
fn package_import_fans_out_to_every_file_in_the_target_directory() {
    // `cmd/server/main.go` imports "github.com/acme/svc/internal/orders"
    // once, but the target directory has two Go files — one edge per
    // file, resolved by longest-suffix match against the walked
    // directory, not `go.mod` parsing.
    let out = TempDir::new("package-import-fanout");
    let graph = index(out.path());

    let imports = imports_edges_from(&graph, "cmd/server/main.go");
    let package_imports: Vec<&&Value> = imports
        .iter()
        .filter(|e| e["evidence"].as_array().unwrap()[0] == "package-import")
        .collect();
    assert_eq!(package_imports.len(), 2);
    assert!(package_imports.iter().all(|e| e["confidence"] == "certain"));

    let targets: Vec<String> = package_imports
        .iter()
        .map(|e| id_to_name(&graph, e["to"].as_str().unwrap()))
        .collect();
    assert!(targets.contains(&"internal/orders/order.go".to_string()));
    assert!(targets.contains(&"internal/orders/helpers.go".to_string()));
}

#[test]
fn unmatched_import_paths_produce_full_path_external_module_nodes() {
    let out = TempDir::new("external-modules");
    let graph = index(out.path());

    let module_paths: Vec<&str> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "module")
        .map(|n| n["path"].as_str().unwrap())
        .collect();
    assert!(module_paths.contains(&"fmt"));
    assert!(module_paths.contains(&"github.com/lib/pq"));

    let imports = imports_edges_from(&graph, "cmd/server/main.go");
    let external: Vec<&&Value> = imports
        .iter()
        .filter(|e| e["evidence"].as_array().unwrap()[0] == "external-package")
        .collect();
    assert_eq!(external.len(), 2, "fmt and github.com/lib/pq");
    assert!(external.iter().all(|e| e["confidence"] == "certain"));
}

#[test]
fn every_symbol_has_a_contains_edge_with_the_go_extractor_origin() {
    let out = TempDir::new("contains");
    let graph = index(out.path());

    let contains: Vec<&Value> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "contains")
        .collect();
    assert_eq!(contains.len(), symbols(&graph).len());
    assert!(
        contains
            .iter()
            .all(|e| e["evidence"].as_array().unwrap() == &[Value::String("lang-go@1".into())])
    );
}

/// Spec §9.5 gate 3 (INV-7), same pattern as every other extractor's
/// determinism test.
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

/// Proves the query layer needed no changes for the seventh language
/// either: `where`/`deps`/`map` work against `fixtures/go-svc` with no
/// go-svc-specific code anywhere in `crates/carto-core/src/query/`.
#[test]
fn where_deps_and_map_work_against_the_go_fixture_unmodified() {
    let out = TempDir::new("query-layer");
    index(out.path());

    let where_output = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("ParseOrder")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .output()
        .unwrap();
    assert!(where_output.status.success());
    let stdout = String::from_utf8(where_output.stdout).unwrap();
    assert!(stdout.contains("ParseOrder"), "got: {stdout}");

    Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("ParseOrder")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();

    Command::cargo_bin("carto")
        .unwrap()
        .arg("map")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();
}
