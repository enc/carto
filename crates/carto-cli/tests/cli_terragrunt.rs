//! CLI-level acceptance tests for ADR-0043's Terragrunt support, against
//! `fixtures/terragrunt-live` (see its README for the scenario matrix).
//! Semantic assertions on the parsed `graph.json` and real CLI output.

use assert_cmd::Command;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/terragrunt-live")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-terragrunt-test-{tag}-{}-{:?}",
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

fn index(out: &Path) -> String {
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(fixture_path())
        .arg("--out")
        .arg(out)
        .assert()
        .success();
    std::fs::read_to_string(out.join("graph.json")).unwrap()
}

fn graph(tag: &str) -> (TempDir, String, Value) {
    let out = TempDir::new(tag);
    let raw = index(out.path());
    let v = serde_json::from_str(&raw).unwrap();
    (out, raw, v)
}

/// `label` of a node id: a file's path, a module's path, or a symbol's
/// `name@file`.
fn labels(graph: &Value) -> BTreeMap<String, String> {
    let nodes = graph["nodes"].as_array().unwrap();
    let files: BTreeMap<&str, &str> = nodes
        .iter()
        .filter(|n| n["kind"] == "file")
        .map(|n| (n["id"].as_str().unwrap(), n["path"].as_str().unwrap()))
        .collect();
    nodes
        .iter()
        .filter_map(|n| {
            let id = n["id"].as_str()?.to_string();
            match n["kind"].as_str()? {
                "file" | "module" => Some((id, n["path"].as_str()?.to_string())),
                "symbol" => Some((
                    id,
                    format!("{}@{}", n["name"].as_str()?, files[n["file"].as_str()?]),
                )),
                _ => None,
            }
        })
        .collect()
}

/// `(from, to, confidence, evidence)` for every edge of `kind` whose
/// first evidence entry starts with `prefix`.
fn edges(graph: &Value, kind: &str, prefix: &str) -> Vec<(String, String, String, Vec<String>)> {
    let l = labels(graph);
    let mut v: Vec<_> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind)
        .filter(|e| {
            e["evidence"][0]
                .as_str()
                .is_some_and(|s| s.starts_with(prefix))
        })
        .map(|e| {
            (
                l[e["from"].as_str().unwrap()].clone(),
                l[e["to"].as_str().unwrap()].clone(),
                e["confidence"].as_str().unwrap().to_string(),
                e["evidence"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    v.sort();
    v
}

const APP: &str = "infra/live/prod/app/terragrunt.hcl";
const VPC: &str = "infra/live/prod/vpc/terragrunt.hcl";

#[test]
fn terraform_source_fans_out_to_the_module_directory_certain() {
    let (_o, _r, g) = graph("source");
    let imports = edges(&g, "imports", "tg-terraform-source");
    let to_of = |from: &str| -> Vec<String> {
        imports
            .iter()
            .filter(|(f, _, c, ev)| {
                f == from && c == "certain" && ev == &vec!["tg-terraform-source".to_string()]
            })
            .map(|(_, t, _, _)| t.clone())
            .collect()
    };
    // `../../../modules//vpc` (`//` collapsed).
    assert_eq!(
        to_of(VPC),
        vec![
            "infra/modules/vpc/outputs.tf".to_string(),
            "infra/modules/vpc/variables.tf".to_string()
        ]
    );
    // `${get_terragrunt_dir()}/../../../modules/app`.
    assert_eq!(
        to_of(APP),
        vec![
            "infra/modules/app/main.tf".to_string(),
            "infra/modules/app/variables.tf".to_string()
        ]
    );
    // `${get_repo_root()}/…` is not statically decidable: no edge at all.
    assert!(
        !imports
            .iter()
            .any(|(f, _, _, _)| f == "infra/live/prod/repo-root/terragrunt.hcl")
    );
}

#[test]
fn dependency_and_include_edges() {
    let (_o, _r, g) = graph("deps");
    let imports = edges(&g, "imports", "tg-");

    // config_path + dependencies.paths both name vpc: one edge, both evidence.
    let dep: Vec<_> = imports
        .iter()
        .filter(|(f, t, _, _)| f == APP && t == VPC)
        .collect();
    assert_eq!(dep.len(), 1, "{dep:?}");
    assert_eq!(dep[0].2, "certain");
    assert!(dep[0].3.contains(&"tg-dependency".to_string()));
    assert!(dep[0].3.contains(&"tg-dependencies".to_string()));

    // find_in_parent_folders is a lookup over walked files: inferred.
    for unit in [APP, VPC] {
        let inc: Vec<_> = imports
            .iter()
            .filter(|(f, t, _, _)| f == unit && t == "infra/root.hcl")
            .collect();
        assert_eq!(inc.len(), 1, "{unit}: {inc:?}");
        assert_eq!(inc[0].2, "inferred");
        assert_eq!(
            inc[0].3,
            vec!["tg-include:find_in_parent_folders".to_string()]
        );
    }
}

#[test]
fn inputs_and_dependency_outputs_reach_the_source_module() {
    let (_o, _r, g) = graph("inputs");
    let refs = edges(&g, "references", "tg-");
    let has = |from: &str, to: &str, ev: &str| {
        refs.iter()
            .any(|(f, t, c, e)| f == from && t == to && c == "inferred" && e[0] == ev)
    };
    assert!(has(
        APP,
        "var.region@infra/modules/app/variables.tf",
        "tg-input:region"
    ));
    assert!(has(
        APP,
        "var.vpc_id@infra/modules/app/variables.tf",
        "tg-input:vpc_id"
    ));
    assert!(has(
        VPC,
        "var.cidr@infra/modules/vpc/variables.tf",
        "tg-input:cidr"
    ));
    assert!(has(
        VPC,
        "var.region@infra/modules/vpc/variables.tf",
        "tg-input:region"
    ));
    // Keys the module declares no variable for are dropped silently.
    assert!(
        !refs
            .iter()
            .any(|(_, _, _, e)| e[0] == "tg-input:nope" || e[0] == "tg-input:unused")
    );
    // root.hcl has no `terraform.source`: its inputs link nowhere.
    assert!(!refs.iter().any(|(f, _, _, _)| f == "infra/root.hcl"));
    // `dependency.vpc.outputs.vpc_id` -> the *vpc unit's* module output.
    assert!(has(
        APP,
        "output.vpc_id@infra/modules/vpc/outputs.tf",
        "tg-dependency-output"
    ));
}

fn unresolved_of(graph: &Value, name: &str, file: &str) -> Vec<String> {
    let file_id = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "file" && n["path"] == file)
        .and_then(|n| n["id"].as_str())
        .unwrap();
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "symbol" && n["name"] == name && n["file"] == file_id)
        .and_then(|n| n["unresolved_calls"].as_array())
        .map(|a| {
            a.iter()
                .map(|u| u["name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_else(|| panic!("no symbol {name} in {file}"))
}

#[test]
fn unresolvable_pieces_are_reported_or_silent_never_guessed() {
    let (_o, _r, g) = graph("unresolved");
    assert_eq!(
        unresolved_of(&g, "local.missing_out", APP),
        vec!["dependency.vpc.outputs.does_not_exist"]
    );
    assert_eq!(
        unresolved_of(&g, "dependency.ghost", APP),
        vec!["config_path:infra/live/prod/ghost"]
    );
    // `${get_repo_root()}` config_path: no claim either way.
    assert!(unresolved_of(&g, "dependency.abs", APP).is_empty());
}

#[test]
fn terragrunt_locals_are_file_local() {
    let (_o, _r, g) = graph("locals");
    let file_of = |label: &str| label.split_once('@').unwrap().1.to_string();
    for (from, to, _, _) in edges(&g, "references", "") {
        if to.starts_with("local.") && from.contains('@') {
            assert_eq!(file_of(&from), file_of(&to), "{from} -> {to}");
        }
    }
    // The root's own `inputs = { region = local.region }` (file scope)
    // does resolve inside root.hcl.
    assert!(
        edges(&g, "references", "")
            .iter()
            .any(|(f, t, _, _)| { f == "infra/root.hcl" && t == "local.region@infra/root.hcl" })
    );
}

#[test]
fn download_caches_are_pruned_and_the_lock_file_yields_no_symbols() {
    let (_o, raw, g) = graph("prune");
    let names: Vec<String> = labels(&g).into_values().collect();
    assert!(!names.iter().any(|n| n.contains(".terragrunt-cache")));
    assert!(!names.iter().any(|n| n.contains("/.terraform/")));
    assert!(!raw.contains("cache_only"));
    assert!(!raw.contains("dot_terraform_only"));
    // `.terraform.lock.hcl` is walked but declares no symbol.
    assert!(names.contains(&"infra/live/prod/vpc/.terraform.lock.hcl".to_string()));
    assert!(
        !names
            .iter()
            .any(|n| n.ends_with("@infra/live/prod/vpc/.terraform.lock.hcl"))
    );
}

#[test]
fn remote_unit_source_is_an_external_module_without_credentials() {
    let (_o, raw, g) = graph("remote");
    let remote = edges(&g, "imports", "tg-terraform-source:remote");
    assert_eq!(remote.len(), 1);
    assert_eq!(remote[0].0, "infra/live/prod/remote/terragrunt.hcl");
    assert_eq!(
        remote[0].1,
        "git::https://example.com/org/net.git//modules/net"
    );
    assert_eq!(remote[0].2, "certain");
    for leaked in ["tok3n", "S3CR3T", "ci-user", "v1.2.0"] {
        assert!(!raw.contains(leaked), "graph.json leaks {leaked}");
    }
}

#[test]
fn map_infra_shows_unit_dependencies_and_graph_is_deterministic() {
    let a = TempDir::new("det-a");
    let b = TempDir::new("det-b");
    assert_eq!(index(a.path()), index(b.path()), "INV-7");

    let stdout = map_infra(a.path());
    assert!(stdout.contains("terragrunt dependencies"), "{stdout}");
    assert!(
        stdout.contains("infra/live/prod/app -> infra/live/prod/vpc"),
        "{stdout}"
    );
    assert!(
        stdout.contains("infra/live/prod/app -> infra/modules/app"),
        "{stdout}"
    );
}

fn map_infra(out: &Path) -> String {
    let stdout = Command::cargo_bin("carto")
        .unwrap()
        .args(["map", "--section", "infra"])
        .arg(fixture_path())
        .arg("--out")
        .arg(out)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(stdout).unwrap()
}
