//! CLI-level acceptance tests for the C# extractor (ADR-0016 — the
//! first language added after spec §5.2's v1 set closed), against
//! `fixtures/csharp-app` (see its own README for the exact scenario
//! matrix). Mirrors `cli_extraction_go.rs`'s structure/style — semantic
//! assertions on parsed JSON rather than a byte-for-byte golden file.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/csharp-app")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-extraction-csharp-test-{tag}-{}-{:?}",
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
        ("Program", "class"),
        ("Main", "method"),
        ("OrderLine", "class"), // record
        ("Money", "struct"),    // record struct
        ("Status", "enum"),
        ("Order", "class"),
        ("DefaultStatus", "const"),
        ("Summary", "method"),
        ("Describe", "method"),
        ("Stamp", "method"),
        ("OrderParser", "class"),
        ("Parse", "method"),
        ("Normalize", "method"),
        ("IAuditSink", "interface"),
        ("Write", "method"),
        ("AuditHook", "type"), // delegate
        ("AuditLog", "class"),
        ("Record", "method"),
        ("Flush", "method"),
    ];
    for (name, sym_kind) in expected {
        let sym = symbol_named(&graph, name);
        assert_eq!(sym["sym_kind"], *sym_kind, "wrong sym_kind for `{name}`");
    }
    // Also proves the deliberate non-symbols: no constructor symbol
    // (exactly one `Order`, counted above) and no `Id` field symbol.
    assert_eq!(
        symbols(&graph).len(),
        expected.len(),
        "unexpected extra/missing symbols"
    );
}

#[test]
fn calls_resolve_via_every_reachable_tier_with_inferred_confidence() {
    let out = TempDir::new("calls");
    let graph = index(out.path());

    let cases: &[(&str, &str, &str)] = &[
        // Constructor body's call, attributed to the class symbol
        // (innermost containment; the constructor is not a symbol).
        ("Order", "Stamp", "same-file"),
        ("Summary", "Describe", "same-file"),
        ("Record", "Flush", "same-file"),
        ("Parse", "Normalize", "same-file"),
        // `new Parser()` — object creation through the alias `using`:
        // tier (b), and the target class is `internal` (counts as
        // exported, ADR-0016).
        ("Main", "OrderParser", "imported"),
        // `new Order(...)` — object creation resolving to the *type*
        // cross-file; tier (c), since a plain namespace `using` never
        // feeds tier (b).
        ("Parse", "Order", "same-package"),
        ("Parse", "Record", "same-package"),
        ("Main", "Parse", "same-package"),
        ("Main", "Summary", "same-package"),
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

    let unresolved = |name: &str| -> Vec<String> {
        symbol_named(&graph, name)["unresolved_calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(unresolved("Main"), vec!["WriteLine"]);
    assert_eq!(unresolved("Parse"), vec!["NewGuid"]);
    assert_eq!(unresolved("Normalize"), vec!["Trim"]);
}

#[test]
fn namespace_using_fans_out_to_every_file_declaring_the_namespace() {
    // `Program.cs` writes `using Acme.Orders;` once; two files declare
    // that namespace — one edge per declaring file. The alias `using
    // Parser = Acme.Orders.OrderParser;` resolves to one of the same
    // files and dedupes onto the fan-out edge (identical kind/endpoints/
    // evidence), so the count stays 2.
    let out = TempDir::new("namespace-fanout");
    let graph = index(out.path());

    let imports = imports_edges_from(&graph, "Program.cs");
    let namespace_imports: Vec<&&Value> = imports
        .iter()
        .filter(|e| e["evidence"].as_array().unwrap()[0] == "namespace-import")
        .collect();
    assert_eq!(namespace_imports.len(), 2);
    assert!(
        namespace_imports
            .iter()
            .all(|e| e["confidence"] == "certain")
    );

    let targets: Vec<String> = namespace_imports
        .iter()
        .map(|e| id_to_name(&graph, e["to"].as_str().unwrap()))
        .collect();
    assert!(targets.contains(&"Orders/Order.cs".to_string()));
    assert!(targets.contains(&"Orders/OrderParser.cs".to_string()));
}

#[test]
fn using_static_resolves_through_the_fqn_index() {
    // `Auditing/AuditLog.cs`'s `using static Acme.Orders.Order;` names
    // a *type* — RawImport::Qualified, resolved via fqn_to_file with
    // C#'s `.` separator.
    let out = TempDir::new("using-static");
    let graph = index(out.path());

    let imports = imports_edges_from(&graph, "Auditing/AuditLog.cs");
    assert_eq!(imports.len(), 1);
    assert_eq!(
        id_to_name(&graph, imports[0]["to"].as_str().unwrap()),
        "Orders/Order.cs"
    );
    assert_eq!(
        imports[0]["evidence"].as_array().unwrap(),
        &[Value::String("namespace-import".into())]
    );
}

#[test]
fn external_usings_produce_full_string_module_nodes_and_internal_misses_none() {
    let out = TempDir::new("external-modules");
    let graph = index(out.path());

    let module_paths: Vec<&str> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "module")
        .map(|n| n["path"].as_str().unwrap())
        .collect();
    // Full-string identity: `System.Text.Json` is its own node, not
    // collapsed into `System` — and `using Acme.Reports;` (known root,
    // no exact namespace match) produced no node at all (INV-8).
    assert_eq!(module_paths, vec!["System", "System.Text.Json"]);

    let external: Vec<&Value> = imports_edges_from(&graph, "Program.cs")
        .into_iter()
        .filter(|e| e["evidence"].as_array().unwrap()[0] == "external-package")
        .collect();
    assert_eq!(external.len(), 2, "System and System.Text.Json");

    // `Orders/OrderParser.cs`'s own `using System;` dedupes onto the
    // same module node.
    let parser_external: Vec<&Value> = imports_edges_from(&graph, "Orders/OrderParser.cs")
        .into_iter()
        .filter(|e| e["evidence"].as_array().unwrap()[0] == "external-package")
        .collect();
    assert_eq!(parser_external.len(), 1);
    assert_eq!(
        id_to_name(&graph, parser_external[0]["to"].as_str().unwrap()),
        "System"
    );
}

#[test]
fn every_symbol_has_a_contains_edge_with_the_csharp_extractor_origin() {
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
            .all(|e| e["evidence"].as_array().unwrap() == &[Value::String("lang-csharp@1".into())])
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

/// Proves the query layer needed no changes for the eighth language
/// either: `where`/`deps`/`map` work against `fixtures/csharp-app` with
/// no csharp-specific code anywhere in `crates/carto-core/src/query/`.
#[test]
fn where_deps_and_map_work_against_the_csharp_fixture_unmodified() {
    let out = TempDir::new("query-layer");
    index(out.path());

    let where_output = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("Parse")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .output()
        .unwrap();
    assert!(where_output.status.success());
    let stdout = String::from_utf8(where_output.stdout).unwrap();
    assert!(stdout.contains("Parse"), "got: {stdout}");

    Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("Parse")
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
