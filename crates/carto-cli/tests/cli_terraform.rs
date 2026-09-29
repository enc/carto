//! CLI-level acceptance tests for ADR-0041's Terraform source support
//! (symbols named by Terraform address + directory-scoped `references`),
//! against `fixtures/tf-modules` (see its README for the scenario
//! matrix). Semantic assertions on the parsed `graph.json` and real CLI
//! output, not a byte-for-byte golden file.

use assert_cmd::Command;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/tf-modules")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-terraform-test-{tag}-{}-{:?}",
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

/// `(name, file path)` per symbol node ID.
fn symbols(graph: &Value) -> BTreeMap<String, (String, String)> {
    let files: BTreeMap<&str, &str> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "file")
        .map(|n| (n["id"].as_str().unwrap(), n["path"].as_str().unwrap()))
        .collect();
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "symbol")
        .map(|n| {
            (
                n["id"].as_str().unwrap().to_string(),
                (
                    n["name"].as_str().unwrap().to_string(),
                    files[n["file"].as_str().unwrap()].to_string(),
                ),
            )
        })
        .collect()
}

/// `(from name@file, to name@file, confidence, evidence)` for every
/// `references` edge between two symbols.
fn references(graph: &Value) -> Vec<(String, String, String, Vec<String>)> {
    let syms = symbols(graph);
    let label = |id: &str| syms.get(id).map(|(n, f)| format!("{n}@{f}"));
    let mut v: Vec<_> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "references")
        .filter_map(|e| {
            Some((
                label(e["from"].as_str()?)?,
                label(e["to"].as_str()?)?,
                e["confidence"].as_str()?.to_string(),
                e["evidence"]
                    .as_array()?
                    .iter()
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect(),
            ))
        })
        .collect();
    v.sort();
    v
}

const PROD: &str = "infra/envs/prod";

#[test]
fn every_top_level_declaration_is_a_symbol_named_by_its_address() {
    let out = TempDir::new("symbols");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    let names: Vec<String> = symbols(&graph)
        .values()
        .map(|(n, f)| format!("{n}@{f}"))
        .collect();
    for expected in [
        format!("var.region@{PROD}/variables.tf"),
        format!("var.name_suffix@{PROD}/variables.tf"),
        format!("var.name_suffix@{PROD}/override.tf"),
        format!("local.prefix@{PROD}/locals.tf.simu"),
        format!("local.prefix@{PROD}/locals.tf.prod"),
        format!("local.bucket_name@{PROD}/main.tf"),
        format!("aws_s3_bucket.logs@{PROD}/main.tf"),
        format!("aws_s3_bucket_policy.logs@{PROD}/main.tf"),
        format!("output.bucket_arn@{PROD}/main.tf"),
        "aws_vpc.main@infra/modules/vpc/main.tf".to_string(),
        "output.vpc_id@infra/modules/vpc/outputs.tf".to_string(),
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
}

#[test]
fn references_stay_inside_their_own_module_directory() {
    let out = TempDir::new("scope");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    let refs = references(&graph);

    // `var.region` is declared in three directories; each use resolves
    // only to its own directory's declaration.
    let region: Vec<_> = refs
        .iter()
        .filter(|(_, to, _, _)| to.starts_with("var.region@"))
        .map(|(from, to, _, _)| (from.as_str(), to.as_str()))
        .collect();
    assert!(region.contains(&(
        "aws_vpc.main@infra/modules/vpc/main.tf",
        "var.region@infra/modules/vpc/variables.tf"
    )));
    assert!(region.contains(&(
        "aws_lambda_function.handler@infra/modules/app/main.tf",
        "var.region@infra/modules/app/variables.tf"
    )));
    assert!(region.contains(&(
        "aws_s3_bucket.logs@infra/envs/prod/main.tf",
        "var.region@infra/envs/prod/variables.tf"
    )));
    assert_eq!(region.len(), 3, "no cross-directory match: {region:?}");
}

#[test]
fn every_reference_edge_is_inferred_never_certain() {
    let out = TempDir::new("confidence");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    let refs = references(&graph);
    assert!(!refs.is_empty());
    assert!(refs.iter().all(|(_, _, c, _)| c == "inferred"), "{refs:?}");
}

#[test]
fn env_variant_locals_fan_out_and_override_yields_to_base() {
    let out = TempDir::new("variants");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    let refs = references(&graph);

    let from = format!("local.bucket_name@{PROD}/main.tf");
    let prefix: Vec<_> = refs
        .iter()
        .filter(|(f, t, _, _)| *f == from && t.starts_with("local.prefix@"))
        .collect();
    assert_eq!(prefix.len(), 2, "one edge per variant: {prefix:?}");
    for variant in ["simu", "prod"] {
        assert!(
            prefix
                .iter()
                .any(|(_, to, _, ev)| to.ends_with(&format!(".tf.{variant}"))
                    && ev.contains(&"tf-ref:env-variant".to_string())
                    && ev.contains(&format!("variant:{variant}"))),
            "{variant}: {prefix:?}"
        );
    }

    // `var.name_suffix` is declared in variables.tf and again in
    // override.tf — resolves to the base declaration only.
    let suffix: Vec<_> = refs
        .iter()
        .filter(|(f, t, _, _)| *f == from && t.starts_with("var.name_suffix@"))
        .collect();
    assert_eq!(suffix.len(), 1, "{suffix:?}");
    assert!(suffix[0].1.ends_with("variables.tf"), "{suffix:?}");
}

#[test]
fn undefined_local_is_reported_and_iterators_are_not() {
    let out = TempDir::new("unresolved");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    let unresolved_of = |name: &str| -> Vec<String> {
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["kind"] == "symbol" && n["name"] == name)
            .and_then(|n| n["unresolved_calls"].as_array())
            .map(|a| {
                a.iter()
                    .map(|u| u["name"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(unresolved_of("local.broken_name"), vec!["local.missing"]);
    // The `dynamic` iterator `statement.value` is not a reference.
    assert!(unresolved_of("aws_s3_bucket_policy.logs").is_empty());
    // No edge was invented for the miss.
    let refs = references(&graph);
    assert!(
        !refs
            .iter()
            .any(|(f, _, _, _)| f.starts_with("local.broken_name@")),
        "{refs:?}"
    );
}

#[test]
fn where_and_deps_work_on_terraform_addresses() {
    let out = TempDir::new("cli");
    index(out.path());

    let where_out = Command::cargo_bin("carto")
        .unwrap()
        .args(["where", "var.region", "--exact"])
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let where_out = String::from_utf8(where_out).unwrap();
    assert_eq!(where_out.matches("tf_variable").count(), 3, "{where_out}");

    let deps_out = Command::cargo_bin("carto")
        .unwrap()
        .args(["deps", "var.region", "--dir", "in"])
        .args(["--subpath", "infra/modules/vpc"])
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let deps_out = String::from_utf8(deps_out).unwrap();
    assert!(deps_out.contains("references aws_vpc.main"), "{deps_out}");
}

#[test]
fn graph_is_deterministic_and_leaks_no_declaration_values() {
    let a = TempDir::new("det-a");
    let b = TempDir::new("det-b");
    let ga = index(a.path());
    let gb = index(b.path());
    assert_eq!(ga, gb, "INV-7: same tree, byte-identical graph.json");
    // Signatures are the block header only; a variable's `default` never
    // reaches the graph.
    assert!(!ga.contains("eu-central-1"));
    assert!(!ga.contains("\"override\""));
}
