use super::super::Component;
use super::*;

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-components-deps-test-{tag}-{}-{:?}",
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

fn component(name: &str, path: &str, kind: &str) -> Component {
    Component {
        name: name.to_string(),
        path: path.to_string(),
        kind: kind.to_string(),
        depends_on: vec![],
    }
}

// --- Go --------------------------------------------------------------

#[test]
fn go_require_matches_another_components_module_declaration() {
    let dir = TempDir::new("go-require");
    std::fs::create_dir_all(dir.path().join("services/orders")).unwrap();
    std::fs::create_dir_all(dir.path().join("libs/shared")).unwrap();
    std::fs::write(
        dir.path().join("services/orders/go.mod"),
        "module example.com/orders\n\ngo 1.22\n\nrequire example.com/shared v0.0.0\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("libs/shared/go.mod"),
        "module example.com/shared\n\ngo 1.22\n",
    )
    .unwrap();
    let mut components = vec![
        component("orders", "services/orders", "go"),
        component("shared", "libs/shared", "go"),
    ];
    resolve(dir.path(), &mut components);
    let orders = components.iter().find(|c| c.name == "orders").unwrap();
    assert_eq!(orders.depends_on, vec!["shared".to_string()]);
    let shared = components.iter().find(|c| c.name == "shared").unwrap();
    assert!(shared.depends_on.is_empty());
}

#[test]
fn go_local_replace_directive_resolves_by_path() {
    let dir = TempDir::new("go-replace");
    std::fs::create_dir_all(dir.path().join("services/orders")).unwrap();
    std::fs::create_dir_all(dir.path().join("libs/shared")).unwrap();
    std::fs::write(
        dir.path().join("services/orders/go.mod"),
        "module example.com/orders\n\ngo 1.22\n\nrequire other.example.com/shared v0.0.0\n\nreplace other.example.com/shared => ../../libs/shared\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("libs/shared/go.mod"),
        "module unrelated-module-path\n\ngo 1.22\n",
    )
    .unwrap();
    let mut components = vec![
        component("orders", "services/orders", "go"),
        component("shared", "libs/shared", "go"),
    ];
    resolve(dir.path(), &mut components);
    let orders = components.iter().find(|c| c.name == "orders").unwrap();
    assert_eq!(orders.depends_on, vec!["shared".to_string()]);
}

#[test]
fn go_require_block_syntax_is_parsed() {
    let content = "module x\n\nrequire (\n\tfoo/bar v1.0.0\n\tbaz/qux v2.0.0\n)\n";
    let info = parse_go_mod(content);
    assert_eq!(info.module, Some("x".to_string()));
    assert_eq!(
        info.requires,
        vec!["foo/bar".to_string(), "baz/qux".to_string()]
    );
}

#[test]
fn go_require_on_a_third_party_module_produces_no_dependency() {
    let dir = TempDir::new("go-external");
    std::fs::create_dir_all(dir.path().join("services/orders")).unwrap();
    std::fs::write(
        dir.path().join("services/orders/go.mod"),
        "module example.com/orders\n\ngo 1.22\n\nrequire github.com/acme/widgets v1.0.0\n",
    )
    .unwrap();
    let mut components = vec![component("orders", "services/orders", "go")];
    resolve(dir.path(), &mut components);
    assert!(components[0].depends_on.is_empty());
}

// --- Node --------------------------------------------------------------

#[test]
fn node_file_protocol_dependency_resolves_by_path() {
    let dir = TempDir::new("node-file-proto");
    std::fs::create_dir_all(dir.path().join("web/admin")).unwrap();
    std::fs::create_dir_all(dir.path().join("libs/ui")).unwrap();
    std::fs::write(
        dir.path().join("web/admin/package.json"),
        r#"{"name": "admin", "dependencies": {"ui": "file:../../libs/ui"}}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("libs/ui/package.json"), r#"{"name": "ui"}"#).unwrap();
    let mut components = vec![
        component("admin", "web/admin", "node"),
        component("ui", "libs/ui", "node"),
    ];
    resolve(dir.path(), &mut components);
    let admin = components.iter().find(|c| c.name == "admin").unwrap();
    assert_eq!(admin.depends_on, vec!["ui".to_string()]);
}

#[test]
fn node_dependency_name_matches_another_components_declared_package_name() {
    let dir = TempDir::new("node-name-match");
    std::fs::create_dir_all(dir.path().join("web/admin")).unwrap();
    std::fs::create_dir_all(dir.path().join("libs/ui")).unwrap();
    std::fs::write(
        dir.path().join("web/admin/package.json"),
        r#"{"name": "admin", "dependencies": {"@acme/ui": "^1.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("libs/ui/package.json"),
        r#"{"name": "@acme/ui"}"#,
    )
    .unwrap();
    let mut components = vec![
        component("admin", "web/admin", "node"),
        component("ui", "libs/ui", "node"),
    ];
    resolve(dir.path(), &mut components);
    let admin = components.iter().find(|c| c.name == "admin").unwrap();
    assert_eq!(admin.depends_on, vec!["ui".to_string()]);
}

#[test]
fn node_ordinary_external_dependency_produces_no_component_dependency() {
    let dir = TempDir::new("node-external");
    std::fs::create_dir_all(dir.path().join("web/admin")).unwrap();
    std::fs::write(
        dir.path().join("web/admin/package.json"),
        r#"{"name": "admin", "dependencies": {"react": "^18.0.0"}}"#,
    )
    .unwrap();
    let mut components = vec![component("admin", "web/admin", "node")];
    resolve(dir.path(), &mut components);
    assert!(components[0].depends_on.is_empty());
}

// --- Rust --------------------------------------------------------------

#[test]
fn rust_inline_table_path_dependency_resolves() {
    let dir = TempDir::new("rust-inline");
    std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
    std::fs::create_dir_all(dir.path().join("crates/b")).unwrap();
    std::fs::write(
        dir.path().join("crates/a/Cargo.toml"),
        "[package]\nname = \"a\"\n\n[dependencies]\nb = { path = \"../b\" }\n",
    )
    .unwrap();
    let mut components = vec![
        component("a", "crates/a", "rust"),
        component("b", "crates/b", "rust"),
    ];
    resolve(dir.path(), &mut components);
    let a = components.iter().find(|c| c.name == "a").unwrap();
    assert_eq!(a.depends_on, vec!["b".to_string()]);
}

#[test]
fn rust_subtable_path_dependency_resolves() {
    let dir = TempDir::new("rust-subtable");
    std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
    std::fs::create_dir_all(dir.path().join("crates/b")).unwrap();
    std::fs::write(
        dir.path().join("crates/a/Cargo.toml"),
        "[package]\nname = \"a\"\n\n[dependencies.b]\npath = \"../b\"\nversion = \"0.1\"\n",
    )
    .unwrap();
    let mut components = vec![
        component("a", "crates/a", "rust"),
        component("b", "crates/b", "rust"),
    ];
    resolve(dir.path(), &mut components);
    let a = components.iter().find(|c| c.name == "a").unwrap();
    assert_eq!(a.depends_on, vec!["b".to_string()]);
}

#[test]
fn rust_inline_table_path_dependency_resolves_when_the_dependency_name_contains_the_key_as_a_substring()
 {
    // `line.find("path")` would previously match inside `path_two`
    // itself (the dependency's own name), before the real `path =`
    // attribute later on the same line -- the real attribute must
    // still be found, not hidden behind that earlier false match.
    let dir = TempDir::new("rust-inline-name-collides-with-key");
    std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
    std::fs::create_dir_all(dir.path().join("crates/path_two")).unwrap();
    std::fs::write(
        dir.path().join("crates/a/Cargo.toml"),
        "[package]\nname = \"a\"\n\n[dependencies]\npath_two = { path = \"../path_two\" }\n",
    )
    .unwrap();
    let mut components = vec![
        component("a", "crates/a", "rust"),
        component("path_two", "crates/path_two", "rust"),
    ];
    resolve(dir.path(), &mut components);
    let a = components.iter().find(|c| c.name == "a").unwrap();
    assert_eq!(a.depends_on, vec!["path_two".to_string()]);
}

#[test]
fn extract_quoted_after_does_not_fall_through_a_malformed_key_to_a_substring_colliding_one() {
    // The multi-occurrence retry (added for the `path_two` case above)
    // must not itself fabricate a match: a malformed (unquoted) real
    // `path` value must not let the search fall through to a *different*
    // key later on the same line just because that key's name contains
    // "path" as a substring.
    let line = "foo = { path = something_bareword, ignored_path = \"spurious/value\" }";
    assert_eq!(extract_quoted_after(line, "path"), None);
}

#[test]
fn extract_quoted_after_does_not_match_a_key_that_is_a_prefix_of_a_longer_identifier() {
    // Same failure shape, opposite side: `Include` must not match
    // inside `IncludeAssets` (a real, different MSBuild/NuGet
    // attribute) when the real `Include` attribute is itself malformed.
    let line = "<ProjectReference Include=bad IncludeAssets=\"all\" />";
    assert_eq!(extract_quoted_after(line, "Include"), None);
}

#[test]
fn extract_quoted_after_still_finds_a_standalone_key_after_a_prefix_collision() {
    // The word-boundary check must not be so strict it rejects a real,
    // well-formed key just because an unrelated longer identifier
    // sharing its prefix appears earlier on the same line.
    let line = "<ProjectReference IncludeAssets=\"all\" Include=\"../Other/Other.csproj\" />";
    assert_eq!(
        extract_quoted_after(line, "Include"),
        Some("../Other/Other.csproj".to_string())
    );
}

#[test]
fn dotnet_include_assets_attribute_is_never_mistaken_for_a_project_reference() {
    let dir = TempDir::new("dotnet-include-assets-collision");
    std::fs::create_dir_all(dir.path().join("services/a")).unwrap();
    std::fs::create_dir_all(dir.path().join("services/b")).unwrap();
    std::fs::write(
        dir.path().join("services/a/A.csproj"),
        "<Project>\n  <ItemGroup>\n    <ProjectReference Include=\"..\\b\\B.csproj\" IncludeAssets=\"all\" />\n  </ItemGroup>\n</Project>\n",
    )
    .unwrap();
    let mut components = vec![
        component("a", "services/a", "dotnet"),
        component("b", "services/b", "dotnet"),
    ];
    resolve(dir.path(), &mut components);
    let a = components.iter().find(|c| c.name == "a").unwrap();
    assert_eq!(a.depends_on, vec!["b".to_string()]);
}

#[test]
fn rust_version_only_dependency_produces_no_component_dependency() {
    let dir = TempDir::new("rust-version-only");
    std::fs::create_dir_all(dir.path().join("crates/a")).unwrap();
    std::fs::write(
        dir.path().join("crates/a/Cargo.toml"),
        "[package]\nname = \"a\"\n\n[dependencies]\nserde = \"1\"\n",
    )
    .unwrap();
    let mut components = vec![component("a", "crates/a", "rust")];
    resolve(dir.path(), &mut components);
    assert!(components[0].depends_on.is_empty());
}

// --- .NET ----------------------------------------------------------------

#[test]
fn dotnet_project_reference_resolves_to_another_component() {
    let dir = TempDir::new("dotnet-ref");
    std::fs::create_dir_all(dir.path().join("services/billing")).unwrap();
    std::fs::create_dir_all(dir.path().join("services/shared-dotnet")).unwrap();
    std::fs::write(
        dir.path().join("services/billing/Billing.csproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <ItemGroup>\n    <ProjectReference Include=\"..\\shared-dotnet\\Shared.csproj\" />\n  </ItemGroup>\n</Project>\n",
    )
    .unwrap();
    let mut components = vec![
        component("billing", "services/billing", "dotnet"),
        component("shared-dotnet", "services/shared-dotnet", "dotnet"),
    ];
    resolve(dir.path(), &mut components);
    let billing = components.iter().find(|c| c.name == "billing").unwrap();
    assert_eq!(billing.depends_on, vec!["shared-dotnet".to_string()]);
}

// --- PHP -----------------------------------------------------------------

#[test]
fn php_path_repository_resolves_to_another_component() {
    let dir = TempDir::new("php-path-repo");
    std::fs::create_dir_all(dir.path().join("services/api")).unwrap();
    std::fs::create_dir_all(dir.path().join("libs/shared-php")).unwrap();
    std::fs::write(
        dir.path().join("services/api/composer.json"),
        r#"{
            "name": "acme/api",
            "require": {"acme/shared-php": "*"},
            "repositories": [{"type": "path", "url": "../../libs/shared-php"}]
        }"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("libs/shared-php/composer.json"),
        r#"{"name": "acme/shared-php"}"#,
    )
    .unwrap();
    let mut components = vec![
        component("api", "services/api", "php"),
        component("shared-php", "libs/shared-php", "php"),
    ];
    resolve(dir.path(), &mut components);
    let api = components.iter().find(|c| c.name == "api").unwrap();
    assert_eq!(api.depends_on, vec!["shared-php".to_string()]);
}

// --- Kinds with no manifest-identity concept ------------------------------

#[test]
fn terraform_and_custom_kinds_get_no_dependency_resolution() {
    let dir = TempDir::new("terraform-none");
    let mut components = vec![
        component("infra", "infra", "terraform"),
        component("custom-thing", "somewhere", "custom"),
    ];
    resolve(dir.path(), &mut components);
    assert!(components[0].depends_on.is_empty());
    assert!(components[1].depends_on.is_empty());
}

#[test]
fn a_component_never_depends_on_itself() {
    // A self-referential path (e.g. a redundant `replace` pointing back
    // at the same directory) must not produce a self-edge.
    let dir = TempDir::new("go-self");
    std::fs::create_dir_all(dir.path().join("services/orders")).unwrap();
    std::fs::write(
        dir.path().join("services/orders/go.mod"),
        "module example.com/orders\n\ngo 1.22\n\nrequire example.com/orders v0.0.0\n",
    )
    .unwrap();
    let mut components = vec![component("orders", "services/orders", "go")];
    resolve(dir.path(), &mut components);
    assert!(components[0].depends_on.is_empty());
}
