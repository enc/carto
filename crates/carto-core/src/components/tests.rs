use super::*;
use crate::error::ErrorKind;
use crate::graph::{self, FileNode};
use crate::taint::Provenance;

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-components-test-{tag}-{}-{:?}",
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

fn file_node(path: &str, lang: Lang) -> Node {
    Node::file(
        graph::file_id(path),
        Provenance::Syntactic,
        "walk@1",
        FileNode {
            path: path.to_string(),
            lang,
            size: 0,
            sha256: None,
            skipped: None,
            excluded: None,
        },
    )
}

#[test]
fn no_markers_anywhere_produces_no_components() {
    let dir = TempDir::new("no-markers");
    let files = vec![
        file_node("src/main.rs", Lang::Rust),
        file_node("README.md", Lang::PlainText),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert!(set.is_empty());
    assert_eq!(set.component_of_path("src/main.rs"), None);
}

#[test]
fn root_level_marker_is_never_a_component() {
    // The walked root itself matching a marker must not become "one
    // component spanning everything" — that would silently turn every
    // pre-existing single-project fixture into a false positive and
    // break the stated backward-compatibility guarantee.
    let dir = TempDir::new("root-marker");
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    let files = vec![
        file_node("Cargo.toml", Lang::Other),
        file_node("src/main.rs", Lang::Rust),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert!(set.is_empty());
}

#[test]
fn nested_go_mod_is_detected_as_a_component() {
    let dir = TempDir::new("go-nested");
    std::fs::create_dir_all(dir.path().join("services/orders")).unwrap();
    std::fs::write(dir.path().join("services/orders/go.mod"), "module orders\n").unwrap();
    let files = vec![
        file_node("services/orders/go.mod", Lang::Other),
        file_node("services/orders/main.go", Lang::Go),
        file_node("README.md", Lang::PlainText),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    let c = &set.components()[0];
    assert_eq!(c.name, "orders");
    assert_eq!(c.path, "services/orders");
    assert_eq!(c.kind, "go");
    assert_eq!(
        set.component_of_path("services/orders/main.go"),
        Some("orders")
    );
    assert_eq!(set.component_of_path("README.md"), None);
    // Segment-boundary safety: a sibling directory with a similar
    // prefix must not falsely match.
    assert_eq!(set.component_of_path("services/orders2/x.go"), None);
}

#[test]
fn workspace_only_cargo_toml_is_suppressed_as_aggregator() {
    let dir = TempDir::new("cargo-aggregator");
    std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
    std::fs::write(dir.path().join("crates/a/Cargo.toml"), "[workspace]\n").unwrap();
    let files = vec![
        file_node("crates/a/Cargo.toml", Lang::Other),
        file_node("crates/a/src/lib.rs", Lang::Rust),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert!(set.is_empty());
}

#[test]
fn package_manifest_cargo_toml_is_a_component() {
    let dir = TempDir::new("cargo-package");
    std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
    std::fs::write(
        dir.path().join("crates/a/Cargo.toml"),
        "[package]\nname=\"a\"\n",
    )
    .unwrap();
    let files = vec![
        file_node("crates/a/Cargo.toml", Lang::Other),
        file_node("crates/a/src/lib.rs", Lang::Rust),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].kind, "rust");
    assert_eq!(set.components()[0].name, "a");
}

#[test]
fn npm_workspaces_root_is_suppressed_as_aggregator() {
    let dir = TempDir::new("npm-aggregator");
    std::fs::create_dir_all(dir.path().join("web")).unwrap();
    std::fs::write(
        dir.path().join("web/package.json"),
        r#"{"workspaces": ["packages/*"]}"#,
    )
    .unwrap();
    let files = vec![
        file_node("web/package.json", Lang::Json),
        file_node("web/packages/app/index.ts", Lang::TypeScript),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert!(set.is_empty());
}

#[test]
fn csproj_suffix_marker_is_detected_as_dotnet() {
    let dir = TempDir::new("csproj");
    std::fs::create_dir_all(dir.path().join("Billing")).unwrap();
    std::fs::write(dir.path().join("Billing/Billing.csproj"), "<Project/>").unwrap();
    let files = vec![
        file_node("Billing/Billing.csproj", Lang::Other),
        file_node("Billing/Program.cs", Lang::CSharp),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].kind, "dotnet");
    assert_eq!(set.components()[0].name, "Billing");
}

#[test]
fn at_least_one_tf_file_is_detected_as_terraform_including_two_part_extension() {
    let dir = TempDir::new("terraform");
    let files = vec![
        file_node("infra/alarms.tf", Lang::Hcl),
        file_node("infra/locals.tf.simu", Lang::Hcl),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].kind, "terraform");
    assert_eq!(set.components()[0].name, "infra");
}

#[test]
fn colliding_basenames_are_disambiguated_by_parent_segment_symmetrically() {
    let dir = TempDir::new("collision");
    let files = vec![
        file_node("services/orders/go.mod", Lang::Other),
        file_node("services/orders/main.go", Lang::Go),
        file_node("lambdas/orders/go.mod", Lang::Other),
        file_node("lambdas/orders/main.go", Lang::Go),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    let mut names: Vec<&str> = set.components().iter().map(|c| c.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["lambdas-orders", "services-orders"]);
}

#[test]
fn innermost_component_wins_for_nested_markers() {
    let dir = TempDir::new("nested");
    let files = vec![
        file_node("services/api/go.mod", Lang::Other),
        file_node("services/api/internal/go.mod", Lang::Other),
        file_node("services/api/internal/x.go", Lang::Go),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 2);
    assert_eq!(
        set.component_of_path("services/api/internal/x.go"),
        Some("internal")
    );
}

#[test]
fn declared_root_with_no_marker_is_detected() {
    let dir = TempDir::new("declared");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"roots": [{"name": "ingest", "path": "lambdas/ingest", "kind": "python"}]}"#,
    )
    .unwrap();
    let files = vec![file_node("lambdas/ingest/handler.py", Lang::Python)];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].name, "ingest");
    assert_eq!(set.components()[0].kind, "python");
}

#[test]
fn declared_roots_replace_detection_by_default() {
    let dir = TempDir::new("declared-replaces");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"roots": [{"name": "only-this", "path": "lambdas/ingest"}]}"#,
    )
    .unwrap();
    let files = vec![
        file_node("services/orders/go.mod", Lang::Other),
        file_node("services/orders/main.go", Lang::Go),
        file_node("lambdas/ingest/handler.py", Lang::Python),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].name, "only-this");
}

#[test]
fn detect_true_makes_declared_roots_additive() {
    let dir = TempDir::new("declared-additive");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"detect": true, "roots": [{"name": "ingest", "path": "lambdas/ingest"}]}"#,
    )
    .unwrap();
    let files = vec![
        file_node("services/orders/go.mod", Lang::Other),
        file_node("services/orders/main.go", Lang::Go),
        file_node("lambdas/ingest/handler.py", Lang::Python),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    let mut names: Vec<&str> = set.components().iter().map(|c| c.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["ingest", "orders"]);
}

#[test]
fn declared_root_overrides_a_detected_component_at_the_same_path() {
    let dir = TempDir::new("declared-override");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"detect": true, "roots": [{"name": "renamed", "path": "services/orders", "kind": "custom-kind"}]}"#,
    )
    .unwrap();
    let files = vec![
        file_node("services/orders/go.mod", Lang::Other),
        file_node("services/orders/main.go", Lang::Go),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].name, "renamed");
    assert_eq!(set.components()[0].kind, "custom-kind");
}

#[test]
fn declared_root_path_with_no_walked_file_is_a_user_error() {
    let dir = TempDir::new("declared-empty-path");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"roots": [{"name": "ghost", "path": "nowhere"}]}"#,
    )
    .unwrap();
    let files = vec![file_node("src/main.rs", Lang::Rust)];
    let err = ComponentSet::discover(dir.path(), &files).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UserError);
}

#[test]
fn declared_root_invalid_name_charset_is_a_user_error() {
    let dir = TempDir::new("declared-bad-name");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"roots": [{"name": "bad name!", "path": "src"}]}"#,
    )
    .unwrap();
    let files = vec![file_node("src/main.rs", Lang::Rust)];
    let err = ComponentSet::discover(dir.path(), &files).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UserError);
}

#[test]
fn declared_root_leading_dotdot_is_a_user_error() {
    let dir = TempDir::new("declared-dotdot");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"roots": [{"name": "escape", "path": "../outside"}]}"#,
    )
    .unwrap();
    let files = vec![file_node("src/main.rs", Lang::Rust)];
    let err = ComponentSet::discover(dir.path(), &files).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UserError);
}

#[test]
fn duplicate_declared_name_for_different_paths_is_a_user_error() {
    let dir = TempDir::new("declared-dup-name");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"roots": [
            {"name": "svc", "path": "a"},
            {"name": "svc", "path": "b"}
        ]}"#,
    )
    .unwrap();
    let files = vec![
        file_node("a/x.rs", Lang::Rust),
        file_node("b/y.rs", Lang::Rust),
    ];
    let err = ComponentSet::discover(dir.path(), &files).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UserError);
}

#[test]
fn malformed_config_json_is_a_user_error() {
    let dir = TempDir::new("declared-malformed");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(dir.path().join(".carto/roots.json"), "{not valid json").unwrap();
    let files = vec![file_node("src/main.rs", Lang::Rust)];
    let err = ComponentSet::discover(dir.path(), &files).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UserError);
}

#[test]
fn missing_config_file_falls_back_to_detection_only() {
    let dir = TempDir::new("no-config-file");
    let files = vec![
        file_node("services/orders/go.mod", Lang::Other),
        file_node("services/orders/main.go", Lang::Go),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
}

#[test]
fn config_digest_changes_with_config_content_and_is_stable_without_one() {
    let dir_a = TempDir::new("digest-a");
    let dir_b = TempDir::new("digest-b");
    let files = vec![file_node("src/main.rs", Lang::Rust)];

    let empty_a = ComponentSet::discover(dir_a.path(), &files).unwrap();
    let empty_b = ComponentSet::discover(dir_b.path(), &files).unwrap();
    assert_eq!(empty_a.config_digest(), empty_b.config_digest());
    assert_eq!(
        empty_a.config_digest(),
        ComponentSet::empty().config_digest()
    );

    std::fs::create_dir_all(dir_a.path().join(".carto")).unwrap();
    std::fs::write(
        dir_a.path().join(".carto/roots.json"),
        r#"{"roots": [{"name": "x", "path": "src"}]}"#,
    )
    .unwrap();
    let with_config = ComponentSet::discover(dir_a.path(), &files).unwrap();
    assert_ne!(with_config.config_digest(), empty_a.config_digest());
}

#[test]
fn sanitizes_non_charset_characters_in_detected_names() {
    let dir = TempDir::new("sanitize");
    let files = vec![
        file_node("services/orders@v2/go.mod", Lang::Other),
        file_node("services/orders@v2/main.go", Lang::Go),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].name, "orders-v2");
}
