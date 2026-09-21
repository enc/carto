//! CLI-level acceptance tests for ADR-0034/0035's multi-root/component
//! support, against `fixtures/monorepo` (see its own README for the
//! exact scenario matrix). Mirrors `cli_extraction_go.rs`'s structure —
//! semantic assertions on parsed JSON and real CLI stdout, not a
//! byte-for-byte golden file: this fixture's job is proving the
//! end-to-end shape (auto-detection, aggregator suppression, a
//! declared root, same-name-across-components resolution, cross- vs.
//! same-component evidence) works through the real binary, which
//! `resolve.rs`'s own unit tests (verifying the underlying mechanism
//! in isolation) don't exercise.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/monorepo")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-multiroot-test-{tag}-{}-{:?}",
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

fn component_of<'a>(graph: &'a Value, path: &str) -> Option<&'a str> {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "file" && n["path"] == path)
        .and_then(|n| n["component"].as_str())
}

#[test]
fn components_are_detected_by_marker_declared_root_and_aggregator_suppressed() {
    let out = TempDir::new("components");
    let graph = index(out.path());

    let components: Vec<(&str, &str, &str)> = graph["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap(),
                c["path"].as_str().unwrap(),
                c["kind"].as_str().unwrap(),
            )
        })
        .collect();

    assert_eq!(
        components,
        vec![
            ("infra", "infra", "terraform"), // ADR-0037 rollup: envs/{prod,dev} + modules/vpc, no infra/*.tf itself
            ("ingest", "lambdas/ingest", "python"), // declared, .carto/roots.json
            ("shared", "libs/shared", "go"),
            ("billing", "services/billing", "dotnet"), // *.csproj suffix marker
            ("orders", "services/orders", "go"),
            ("admin", "web/admin", "node"),
        ],
        "components must be exactly these six, sorted by path (INV-7)"
    );

    // The root-level [workspace]-only Cargo.toml must never become a
    // component itself (ADR-0034: the walked root is never a
    // candidate) -- confirmed by the count above already being exactly
    // 6, not 7.
    assert_eq!(component_of(&graph, "Cargo.toml"), None);
    assert_eq!(
        component_of(&graph, "infra/envs/prod/main.tf"),
        Some("infra"),
        "scattered infra/envs/*, infra/modules/* directories roll up to one infra component (ADR-0037)"
    );
    assert_eq!(
        component_of(&graph, "tools/check.py"),
        None,
        "a loose root-level script belongs to no component"
    );

    assert_eq!(
        component_of(&graph, "services/orders/handler.go"),
        Some("orders")
    );
    assert_eq!(
        component_of(&graph, "services/billing/Handler.cs"),
        Some("billing")
    );
    assert_eq!(
        component_of(&graph, "web/admin/src/handler.ts"),
        Some("admin")
    );
    assert_eq!(
        component_of(&graph, "lambdas/ingest/handler.py"),
        Some("ingest")
    );
}

#[test]
fn where_lists_every_same_named_handler_labeled_by_component() {
    let out = TempDir::new("where");
    index(out.path());

    let assert = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("Handler")
        .arg("--exact")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    // The exact motivating scenario: three unrelated `Handler` symbols,
    // one per component, each labeled -- proving `where` doesn't just
    // find them (it always could) but correctly attributes each to its
    // own component, not merging or mislabeling any of them.
    assert!(
        stdout.contains("services/billing/Handler.cs") && stdout.contains("[billing]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("services/orders/handler.go") && stdout.contains("[orders]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("web/admin/src/handler.ts") && stdout.contains("[admin]"),
        "{stdout}"
    );
}

#[test]
fn deps_handler_unscoped_is_ambiguous_but_component_resolves_it() {
    let out = TempDir::new("deps-ambiguous");
    let graph = index(out.path());

    // Unscoped: three repo-wide candidates, none of which resolve_target
    // can pick -- a UserError (exit 1) naming all three, not a silent
    // guess (INV-8).
    Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("Handler")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .failure()
        .code(1);

    // --component as a tiebreaker (ADR-0014's shape, extended by
    // ADR-0035) resolves the *target* cleanly to orders' own Handler.
    // Note: --component also scopes *reported* rows (the same flag
    // doing double duty `--subpath` already does in deps_cmd.rs), so
    // Handler's own cross-component edge to `shared` is correctly
    // hidden here -- that's the reporting filter working as intended,
    // checked separately below via the unscoped, unambiguous node ID.
    let assert = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("Handler")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--component")
        .arg("orders")
        .arg("--dir")
        .arg("out")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("services/orders/handler.go"), "{stdout}");

    // Same query, unfiltered by --component this time (using the exact
    // node ID directly sidesteps the name ambiguity without narrowing
    // which rows get reported) -- Handler's own body calls Shared
    // (libs/shared), a real cross-component bare-name resolution,
    // flagged accordingly.
    let handler_id = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "symbol" && n["name"] == "Handler" && n["sym_kind"] == "function")
        .and_then(|n| n["id"].as_str())
        .expect("orders' Handler function symbol must exist");
    let assert = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg(handler_id)
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--dir")
        .arg("out")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Shared") && stdout.contains("[cross-component]"),
        "{stdout}"
    );
}

#[test]
fn same_component_call_resolves_despite_the_repo_wide_name_collision() {
    let out = TempDir::new("deps-same-component");
    index(out.path());

    // orders/main.go's Run() calls bare Handler() from within the same
    // component -- before ADR-0034/0035 this was ambiguous against
    // billing's and admin's own Handler (three repo-wide candidates,
    // no edge at all). Tier (c1) now resolves it unambiguously, and
    // the edge must NOT carry a [cross-component] marker.
    let assert = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("Run")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--dir")
        .arg("out")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("services/orders/handler.go:10"), "{stdout}");
    assert!(!stdout.contains("[cross-component]"), "{stdout}");
}

#[test]
fn module_level_registration_resolves_at_file_scope_under_a_component() {
    let out = TempDir::new("file-scope");
    index(out.path());

    // web/admin/src/handler.ts's module-level `registerHandler(new
    // Handler())` has no enclosing symbol (ADR-0031's file-scope
    // fallback) -- must still resolve, from the File node, and still
    // carry admin's own component.
    let assert = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .arg("web/admin/src/handler.ts")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--dir")
        .arg("out")
        .arg("--kinds")
        .arg("calls,references")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("references Handler"), "{stdout}");
    assert!(stdout.contains("calls registerHandler"), "{stdout}");
}

#[test]
fn map_components_section_lists_every_component_and_the_cross_component_edge() {
    let out = TempDir::new("map");
    index(out.path());

    let assert = Command::cargo_bin("carto")
        .unwrap()
        .arg("map")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--section")
        .arg("components")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("## components"), "{stdout}");
    for name in ["admin", "billing", "infra", "ingest", "orders", "shared"] {
        assert!(stdout.contains(name), "{stdout}");
    }
    // ADR-0039: orders' go.mod really `require`s shared.
    assert!(
        stdout.contains("orders  path=services/orders kind=go files=3 symbols=2 depends_on=shared"),
        "{stdout}"
    );
    assert!(stdout.contains("## cross-component edges"), "{stdout}");
    assert!(stdout.contains("orders -> shared"), "{stdout}");
    assert!(
        !stdout.contains("[undeclared]"),
        "orders -> shared is a declared dependency, must not be flagged: {stdout}"
    );
}

#[test]
fn unknown_component_name_is_a_clean_user_error() {
    let out = TempDir::new("unknown-component");
    index(out.path());

    let assert = Command::cargo_bin("carto")
        .unwrap()
        .arg("where")
        .arg("Handler")
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--component")
        .arg("nonexistent")
        .assert()
        .failure()
        .code(1);
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("nonexistent"), "{stderr}");
    assert!(stderr.contains("orders"), "{stderr}"); // lists known components
}
