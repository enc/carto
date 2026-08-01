//! CLI-level acceptance tests for the PHP extractor (ADR-0012), against
//! `fixtures/php-app` (see its README for the exact scenario matrix).
//! Mirrors `cli_extraction_python.rs`'s structure/style — semantic
//! assertions on parsed JSON rather than a byte-for-byte golden file,
//! same reasoning as that file's own doc comment.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/php-app")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-extraction-php-test-{tag}-{}-{:?}",
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

#[test]
fn extracts_every_expected_symbol_with_correct_sym_kind() {
    let out = TempDir::new("symbols");
    let graph = index(out.path());

    let expected: &[(&str, &str)] = &[
        ("Order", "class"),
        ("__construct", "method"),
        ("summary", "method"),
        ("fromArray", "method"),
        ("refresh", "method"),
        ("parseOrder", "function"),
        ("validate", "function"),
        ("auditOrder", "function"),
        ("Handler", "class"),
        ("handle", "method"),
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
fn method_has_qualified_stable_id_distinct_from_a_same_named_function_would_have() {
    let out = TempDir::new("qualified-id");
    let graph = index(out.path());
    // The qualified-name-in-ID recipe (spec §4.3) means `Order::summary`
    // and a hypothetical bare `summary` function would never collide —
    // asserted indirectly here by confirming the method symbol exists
    // and is uniquely identified (`symbol_named` already panics on zero
    // matches; this confirms it isn't a duplicate either — the innermost-
    // symbol call-attribution fix, resolve.rs, doesn't affect symbol
    // extraction itself, only which symbol a call is charged against).
    let matches: Vec<&Value> = symbols(&graph)
        .into_iter()
        .filter(|s| s["name"] == "summary")
        .collect();
    assert_eq!(matches.len(), 1, "method must be extracted exactly once");
}

#[test]
fn calls_resolve_via_every_tier_with_inferred_confidence() {
    let out = TempDir::new("calls");
    let graph = index(out.path());

    let cases: &[(&str, &str, &str)] = &[
        ("parseOrder", "validate", "same-file"),
        ("handle", "parseOrder", "imported"),
        ("handle", "auditOrder", "same-package"),
        ("handle", "summary", "same-package"),
        // Order::fromArray() — a *static* call, deliberately captured
        // (ADR-0012's divergence from ADR-0008's Rust precedent).
        ("handle", "fromArray", "same-package"),
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
    // `refresh` is `private` (no tier b/c candidate); `unknownExternalCall`
    // has no candidate anywhere; `strip` is `$input->strip()`, a stdlib
    // method call with no symbol carto ever sees.
    assert_eq!(unresolved, vec!["refresh", "unknownExternalCall", "strip"]);
}

#[test]
fn enclosing_class_symbol_does_not_also_claim_its_methods_calls() {
    // Regression: resolve.rs used to attribute a call to *every* symbol
    // whose line range contained it, not just the innermost one — a PHP
    // `class` symbol's range spans its methods' bodies too (unlike Rust,
    // where an `impl` block is never itself a symbol), so every call
    // inside `Handler::handle` was double-counted against the `Handler`
    // class symbol as well. Caught by eyeballing real `carto index`
    // output against this fixture, not a synthetic snippet (CLAUDE.md's
    // documented gotcha class).
    let out = TempDir::new("innermost-symbol");
    let graph = index(out.path());

    let handler = symbol_named(&graph, "Handler");
    assert!(
        handler["unresolved_calls"].as_array().unwrap().is_empty(),
        "Handler class symbol must not claim handle()'s unresolved calls"
    );

    let calls_from_handler = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "calls" && e["from"] == handler["id"])
        .count();
    assert_eq!(
        calls_from_handler, 0,
        "Handler class symbol must not claim handle()'s calls edges"
    );
}

#[test]
fn namespace_qualified_use_resolves_via_fqn_index() {
    let out = TempDir::new("namespace-import");
    let graph = index(out.path());

    let imports: Vec<&Value> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "imports")
        .collect();
    assert_eq!(
        imports.len(),
        2,
        "one namespace-import, one external-package"
    );

    let by_evidence = |ev: &str| -> &Value {
        imports
            .iter()
            .find(|e| e["evidence"].as_array().unwrap() == &[Value::String(ev.to_string())])
            .unwrap_or_else(|| panic!("no imports edge with evidence `{ev}`"))
    };

    let namespace_import = by_evidence("namespace-import");
    assert_eq!(namespace_import["confidence"], "certain");
    assert_eq!(
        id_to_name(&graph, namespace_import["to"].as_str().unwrap()),
        "src/Orders.php"
    );

    let external = by_evidence("external-package");
    assert_eq!(external["confidence"], "certain");
    let external_module = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == external["to"])
        .unwrap();
    assert_eq!(external_module["path"], "Psr");
    assert_eq!(external_module["external"], true);
}

#[test]
fn use_with_known_root_but_no_fqn_match_produces_no_edge_no_node() {
    // `use App\Missing\Thing;` -- `App` is a known namespace root (other
    // files declare `namespace App\...;`), but no file declares exactly
    // `App\Missing\Thing`: internal but unresolvable, an honest omission
    // (INV-8), not a guess and not an external module either.
    let out = TempDir::new("unresolvable-namespace-import");
    let graph = index(out.path());

    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["kind"] != "module" || n["path"] != "App"),
        "unresolvable internal namespace import must not produce a module node"
    );
    let imports_from_app_missing = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "imports")
        .count();
    assert_eq!(
        imports_from_app_missing, 2,
        "only the two resolvable use statements produce imports edges"
    );
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
    for edge in graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "contains")
    {
        assert_eq!(
            edge["evidence"].as_array().unwrap(),
            &[Value::String("lang-php@1".to_string())]
        );
    }
}

/// Spec §9.5 gate 3 (INV-7), same pattern as `cli_extraction.rs`'s Rust
/// and `cli_extraction_python.rs`'s Python determinism tests.
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

/// Proves the query layer needed no changes for a third language:
/// `where`/`deps`/`map` work against `fixtures/php-app` with no
/// php-app-specific code anywhere in `crates/carto-core/src/query/`.
#[test]
fn where_deps_and_map_work_against_the_php_fixture_unmodified() {
    let out = TempDir::new("query-layer");
    index(out.path());

    let where_output = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("summary")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .output()
        .unwrap();
    assert!(where_output.status.success());
    let stdout = String::from_utf8(where_output.stdout).unwrap();
    assert!(stdout.contains("summary"), "got: {stdout}");

    Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("handle")
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
