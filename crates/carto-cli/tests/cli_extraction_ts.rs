//! CLI-level acceptance tests for the TypeScript/TSX/JavaScript
//! extractors (ADR-0013), against `fixtures/ts-app` (see its README
//! for the exact scenario matrix). Mirrors `cli_extraction_php.rs`'s
//! structure/style — semantic assertions on parsed JSON rather than a
//! byte-for-byte golden file, same reasoning as that file's own doc
//! comment. Unlike the single-language fixtures, `fixtures/ts-app`
//! mixes three `Lang` variants (`.ts`/`.tsx`/`.js`), so the
//! `contains`-edge-origin check here is per-file rather than one
//! constant.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/ts-app")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-extraction-ts-test-{tag}-{}-{:?}",
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

/// ADR-0029: `parseOrder`'s own `: Order` return-type annotation is a
/// type position nothing captured before this.
#[test]
fn type_annotation_produces_a_references_edge() {
    let out = TempDir::new("references");
    let graph = index(out.path());

    let edge = references_edge(&graph, "parseOrder", "Order");
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
        ("Order", "interface"),
        ("parseOrder", "function"),
        ("validate", "function"),
        ("auditOrder", "function"),
        ("internalHelper", "function"),
        ("logOrder", "function"),
        ("Handler", "class"),
        ("handle", "method"),
        ("legacyHelper", "function"),
        ("Component", "function"),
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
fn top_level_arrow_function_const_is_captured_as_a_function_symbol() {
    let out = TempDir::new("arrow-const");
    let graph = index(out.path());
    let audit_order = symbol_named(&graph, "auditOrder");
    assert_eq!(audit_order["sym_kind"], "function");
    // `is_pub` isn't itself serialized into graph.json — it's proven
    // indirectly here (`auditOrder` is exported, so it's a valid tier
    // (c) candidate) and for the negative case
    // (`internalHelper`, never exported, must NOT be one) in
    // `unresolved_calls_are_recorded_honestly_not_guessed` below, via
    // `calls_resolve_via_every_tier_with_inferred_confidence`'s
    // `handle -> auditOrder` same-package assertion.
}

#[test]
fn calls_resolve_via_every_tier_with_inferred_confidence() {
    let out = TempDir::new("calls");
    let graph = index(out.path());

    let cases: &[(&str, &str, &str)] = &[
        ("parseOrder", "validate", "same-file"),
        ("handle", "parseOrder", "imported"),
        // `v` is `validate as v` — proves ADR-0013's alias-resolution
        // fix end-to-end: before that fix, no aliased import could
        // ever resolve via tier (b) at all.
        ("handle", "validate", "imported"),
        ("handle", "logOrder", "imported"),
        ("handle", "auditOrder", "same-package"),
        ("Component", "legacyHelper", "imported"),
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
    // `internalHelper` is module-private (no `export`) so it's never a
    // tier (b)/(c) candidate; `unknownExternalCall` has no candidate
    // anywhere.
    assert_eq!(unresolved, vec!["internalHelper", "unknownExternalCall"]);
}

#[test]
fn relative_import_crossing_directories_has_a_multi_segment_module_path() {
    // `import { logOrder } from '../shared/logging'` from
    // src/handlers/index.ts — one level up, then back down into a
    // sibling subdirectory. Asserted indirectly: the edge exists and
    // resolves to the right file, proving `module_path`'s internal
    // slash ("shared/logging") was handled correctly end-to-end, not
    // just unit-tested against the extractor in isolation.
    let out = TempDir::new("multi-segment-path");
    let graph = index(out.path());

    let imports: Vec<&Value> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["kind"] == "imports"
                && id_to_name(&graph, e["from"].as_str().unwrap()) == "src/handlers/index.ts"
                && id_to_name(&graph, e["to"].as_str().unwrap()) == "src/shared/logging.ts"
        })
        .collect();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0]["confidence"], "certain");
    assert_eq!(
        imports[0]["evidence"].as_array().unwrap(),
        &[Value::String("relative-import".to_string())]
    );
}

#[test]
fn scoped_package_import_roots_at_the_first_two_segments() {
    let out = TempDir::new("scoped-package");
    let graph = index(out.path());

    let module = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "module")
        .expect("an external module node must exist");
    assert_eq!(module["path"], "@scope/pkg");
    assert_eq!(module["external"], true);
}

#[test]
fn reexport_produces_a_certain_imports_edge() {
    let out = TempDir::new("reexport");
    let graph = index(out.path());

    let imports: Vec<&Value> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["kind"] == "imports"
                && id_to_name(&graph, e["from"].as_str().unwrap()) == "src/reexport.ts"
        })
        .collect();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0]["confidence"], "certain");
    assert_eq!(
        id_to_name(&graph, imports[0]["to"].as_str().unwrap()),
        "src/orders.ts"
    );
}

#[test]
fn cross_lang_variant_import_resolves_tsx_to_js() {
    // src/Component.tsx imports src/legacy.js — proves extension-
    // guessing and cross-Lang-variant resolution both work, not just
    // same-language relative imports.
    let out = TempDir::new("cross-variant");
    let graph = index(out.path());

    let component_file = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "file" && n["path"] == "src/Component.tsx")
        .unwrap();
    assert_eq!(component_file["lang"], "tsx");

    let legacy_file = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "file" && n["path"] == "src/legacy.js")
        .unwrap();
    assert_eq!(legacy_file["lang"], "java_script");

    let edge = calls_edge(&graph, "Component", "legacyHelper");
    assert_eq!(edge["confidence"], "inferred");
}

#[test]
fn every_symbol_has_a_contains_edge_with_the_declaring_files_own_origin() {
    let out = TempDir::new("contains");
    let graph = index(out.path());

    let contains: Vec<&Value> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "contains")
        .collect();
    assert_eq!(contains.len(), symbols(&graph).len());

    // Mixed fixture: each symbol's contains-edge evidence must match
    // its *own* file's extractor origin, not a single constant the
    // way single-language fixtures can assert.
    for edge in &contains {
        let file_path = id_to_name(&graph, edge["from"].as_str().unwrap());
        let expected_origin = if file_path.ends_with(".tsx") {
            "lang-tsx@1"
        } else if file_path.ends_with(".ts") {
            "lang-ts@1"
        } else if file_path.ends_with(".js") {
            "lang-js@1"
        } else {
            panic!("unexpected declaring file for a contains edge: {file_path}")
        };
        assert_eq!(
            edge["evidence"].as_array().unwrap(),
            &[Value::String(expected_origin.to_string())],
            "wrong origin for a symbol declared in {file_path}"
        );
    }
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

/// Proves the query layer needed no changes for a fourth/fifth/sixth
/// language: `where`/`deps`/`map` work against `fixtures/ts-app` with
/// no ts-app-specific code anywhere in `crates/carto-core/src/query/`.
#[test]
fn where_deps_and_map_work_against_the_ts_fixture_unmodified() {
    let out = TempDir::new("query-layer");
    index(out.path());

    let where_output = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("handle")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .output()
        .unwrap();
    assert!(where_output.status.success());
    let stdout = String::from_utf8(where_output.stdout).unwrap();
    assert!(stdout.contains("handle"), "got: {stdout}");

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
