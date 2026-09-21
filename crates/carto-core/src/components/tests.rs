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
            component: None,
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
fn declared_root_name_colliding_with_a_different_detected_component_is_a_user_error() {
    // A declared root that does NOT share the detected component's
    // path (unlike the "override" test above) must not silently
    // produce two distinct components under the same name -- map.rs's
    // per-name BTreeMap would merge their counts, and resolve.rs's
    // component-scoped tiers would cross-match calls between two
    // unrelated directories.
    let dir = TempDir::new("declared-name-collides-with-detected");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"detect": true, "roots": [{"name": "billing", "path": "lambdas/legacy-billing"}]}"#,
    )
    .unwrap();
    let files = vec![
        file_node("services/billing/go.mod", Lang::Other),
        file_node("services/billing/main.go", Lang::Go),
        file_node("lambdas/legacy-billing/handler.py", Lang::Python),
    ];
    let err = ComponentSet::discover(dir.path(), &files).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UserError);
    assert!(err.to_string().contains("billing"));
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
fn declared_root_path_containing_dotdot_only_as_a_substring_is_accepted() {
    // `path.contains("..")` alone would also reject a legitimately
    // named directory like `services/my..lib` even though it never
    // leaves the repo root -- only a real `..` *segment* is a
    // traversal attempt.
    let dir = TempDir::new("declared-dotdot-substring");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"roots": [{"name": "mylib", "path": "services/my..lib"}]}"#,
    )
    .unwrap();
    let files = vec![file_node("services/my..lib/main.go", Lang::Go)];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].path, "services/my..lib");
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

// --- ADR-0037: terraform rollup, aggregator parsing, .carto/roots.json
// `exclude` -----------------------------------------------------------

#[test]
fn terraform_directory_with_its_own_tf_absorbs_a_nested_module_directory() {
    // T1: `infra` itself has a `.tf` file, so `infra/modules/vpc`
    // (nested, also `.tf`-bearing) collapses into it rather than
    // becoming its own "vpc" component.
    let dir = TempDir::new("tf-t1");
    let files = vec![
        file_node("infra/main.tf", Lang::Hcl),
        file_node("infra/modules/vpc/main.tf", Lang::Hcl),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].path, "infra");
    assert_eq!(set.components()[0].kind, "terraform");
}

#[test]
fn scattered_terraform_directories_with_no_tf_bearing_common_ancestor_roll_up_together() {
    // T3: no directory anywhere under `infra` has a `.tf` file of its
    // own (only the leaf module/env directories do) — without rollup
    // this fragments into four separate, generically-named components
    // (`prod`, `dev`, `vpc`, `rds`).
    let dir = TempDir::new("tf-t3");
    let files = vec![
        file_node("infra/envs/prod/main.tf", Lang::Hcl),
        file_node("infra/envs/dev/main.tf", Lang::Hcl),
        file_node("infra/modules/vpc/main.tf", Lang::Hcl),
        file_node("infra/modules/rds/main.tf", Lang::Hcl),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].path, "infra");
    assert_eq!(set.components()[0].kind, "terraform");
    assert_eq!(
        set.component_of_path("infra/envs/prod/main.tf"),
        Some("infra")
    );
}

#[test]
fn terraform_directory_under_a_strong_component_belongs_to_that_component() {
    // T2: `services/orders/infra` has its own `.tf` file, but
    // `services/orders` is already a `go.mod`-anchored component — the
    // terraform directory must not become a second, separate one.
    let dir = TempDir::new("tf-t2");
    let files = vec![
        file_node("services/orders/go.mod", Lang::Other),
        file_node("services/orders/main.go", Lang::Go),
        file_node("services/orders/infra/main.tf", Lang::Hcl),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].path, "services/orders");
    assert_eq!(set.components()[0].kind, "go");
    assert_eq!(
        set.component_of_path("services/orders/infra/main.tf"),
        Some("orders")
    );
}

#[test]
fn two_terraform_trees_with_no_shared_ancestor_short_of_the_repo_root_stay_separate() {
    // Rollup must not go as far as merging genuinely unrelated
    // terraform trees just because both ultimately sit under the
    // walked root — that would collapse them to a "" (repo-root)
    // ancestor, which the rollup rule explicitly refuses.
    let dir = TempDir::new("tf-disjoint");
    let files = vec![
        file_node("infra-a/main.tf", Lang::Hcl),
        file_node("infra-b/main.tf", Lang::Hcl),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    let mut paths: Vec<&str> = set.components().iter().map(|c| c.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["infra-a", "infra-b"]);
}

#[test]
fn pnpm_workspace_sibling_suppresses_package_json_as_aggregator() {
    // A pnpm workspace declares its members in pnpm-workspace.yaml, not
    // in package.json's own "workspaces" key — content-sniffing
    // package.json alone would miss it.
    let dir = TempDir::new("pnpm-aggregator");
    std::fs::create_dir_all(dir.path().join("root")).unwrap();
    std::fs::write(dir.path().join("root/package.json"), r#"{"name": "root"}"#).unwrap();
    std::fs::write(dir.path().join("root/pnpm-workspace.yaml"), "packages:\n").unwrap();
    let files = vec![
        file_node("root/package.json", Lang::Json),
        file_node("root/pnpm-workspace.yaml", Lang::Other),
        file_node("root/packages/app/index.ts", Lang::TypeScript),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert!(set.is_empty());
}

#[test]
fn package_json_mentioning_workspaces_only_in_a_string_value_is_a_real_component() {
    // Real JSON parsing, not a `"workspaces"` substring scan: the word
    // appearing inside an unrelated string value must not suppress a
    // genuine component the way the old substring check would have.
    let dir = TempDir::new("package-json-real-parse");
    std::fs::create_dir_all(dir.path().join("app")).unwrap();
    std::fs::write(
        dir.path().join("app/package.json"),
        r#"{"name": "app", "description": "not related to npm workspaces at all"}"#,
    )
    .unwrap();
    let files = vec![
        file_node("app/package.json", Lang::Json),
        file_node("app/index.ts", Lang::TypeScript),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].kind, "node");
}

#[test]
fn cargo_toml_workspace_metadata_table_is_not_mistaken_for_a_workspace_root() {
    // Line-anchored, not a substring scan: `[workspace.metadata.foo]`
    // is a nested table, not a `[workspace]` header — a real
    // `[package]` component must still be detected.
    let dir = TempDir::new("cargo-nested-table");
    std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
    std::fs::write(
        dir.path().join("crates/a/Cargo.toml"),
        "[package]\nname = \"a\"\n\n[workspace.metadata.foo]\nbar = 1\n",
    )
    .unwrap();
    let files = vec![
        file_node("crates/a/Cargo.toml", Lang::Other),
        file_node("crates/a/src/lib.rs", Lang::Rust),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].kind, "rust");
}

#[test]
fn exclude_drops_a_detected_component() {
    let dir = TempDir::new("exclude-basic");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"exclude": ["infra"]}"#,
    )
    .unwrap();
    let files = vec![file_node("infra/main.tf", Lang::Hcl)];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert!(set.is_empty());
}

#[test]
fn exclude_matching_nothing_is_not_an_error() {
    let dir = TempDir::new("exclude-no-match");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"exclude": ["nowhere/at/all"]}"#,
    )
    .unwrap();
    let files = vec![
        file_node("services/orders/go.mod", Lang::Other),
        file_node("services/orders/main.go", Lang::Go),
    ];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
}

#[test]
fn exclude_invalid_path_is_a_user_error() {
    let dir = TempDir::new("exclude-bad-path");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"exclude": ["../outside"]}"#,
    )
    .unwrap();
    let files = vec![file_node("src/main.rs", Lang::Rust)];
    let err = ComponentSet::discover(dir.path(), &files).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UserError);
}

#[test]
fn declared_root_can_re_add_a_path_the_same_config_excludes() {
    // exclude runs before roots, so a declared root at the same path
    // as an exclude entry still ends up present — exclude only
    // suppresses *auto-detection*, roots is unconditional intent.
    let dir = TempDir::new("exclude-then-readd");
    std::fs::create_dir_all(dir.path().join(".carto")).unwrap();
    std::fs::write(
        dir.path().join(".carto/roots.json"),
        r#"{"detect": true, "exclude": ["infra"], "roots": [{"name": "infra", "path": "infra", "kind": "terraform"}]}"#,
    )
    .unwrap();
    let files = vec![file_node("infra/main.tf", Lang::Hcl)];
    let set = ComponentSet::discover(dir.path(), &files).unwrap();
    assert_eq!(set.components().len(), 1);
    assert_eq!(set.components()[0].name, "infra");
}
