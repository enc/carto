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
        .filter(|(_, to, _, ev)| {
            to.starts_with("var.region@") && ev.contains(&"tf-ref:same-module".to_string())
        })
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
    // The property that matters: every same-module edge stays inside
    // one directory, whatever else the fixture adds.
    let dir_of = |label: &str| -> String {
        let path = label.split_once('@').unwrap().1;
        path.rsplit_once('/').unwrap().0.to_string()
    };
    for (from, to) in &region {
        assert_eq!(
            dir_of(from),
            dir_of(to),
            "cross-directory match: {from} -> {to}"
        );
    }
    for (from, to, _, ev) in &refs {
        if ev.contains(&"tf-ref:same-module".to_string()) {
            assert_eq!(
                dir_of(from),
                dir_of(to),
                "cross-directory match: {from} -> {to}"
            );
        }
    }
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
    // INV-3/§5.1: `*.tfvars` and `*.tfvars.json` contents are never read.
    assert!(!ga.contains("TFVARS-CANARY"));
    assert!(!ga.contains("TFVARS-JSON-CANARY"));
}

// ---- ADR-0042: module calls ------------------------------------------

const MODULES_TF: &str = "infra/envs/prod/modules.tf";

/// `(from path, to label, confidence, evidence)` for every `imports`
/// edge whose first evidence entry starts with `tf-module`.
fn module_imports(graph: &Value) -> Vec<(String, String, String, Vec<String>)> {
    let label: BTreeMap<&str, String> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| match n["kind"].as_str()? {
            "file" => Some((n["id"].as_str()?, n["path"].as_str()?.to_string())),
            "module" => Some((n["id"].as_str()?, n["path"].as_str()?.to_string())),
            _ => None,
        })
        .collect();
    let mut v: Vec<_> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "imports")
        .filter(|e| {
            e["evidence"][0]
                .as_str()
                .is_some_and(|s| s.starts_with("tf-module"))
        })
        .map(|e| {
            (
                label[e["from"].as_str().unwrap()].clone(),
                label[e["to"].as_str().unwrap()].clone(),
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

#[test]
fn local_module_source_imports_every_file_of_the_target_directory_certain() {
    let out = TempDir::new("mod-imports");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    let imports = module_imports(&graph);

    for (dir, files) in [
        (
            "infra/modules/vpc",
            vec!["main.tf", "outputs.tf", "variables.tf"],
        ),
        ("infra/modules/app", vec!["main.tf", "variables.tf"]),
    ] {
        let mut got: Vec<&str> = imports
            .iter()
            .filter(|(f, t, _, _)| f == MODULES_TF && t.starts_with(&format!("{dir}/")))
            .map(|(_, t, c, ev)| {
                assert_eq!(c, "certain");
                assert_eq!(ev, &vec!["tf-module-source".to_string()]);
                t.rsplit('/').next().unwrap()
            })
            .collect();
        got.sort();
        assert_eq!(got, files, "{dir}");
    }
    // The unresolvable `../../modules/nope` produced no edge at all.
    assert!(!imports.iter().any(|(_, t, _, _)| t.contains("nope")));
}

#[test]
fn remote_module_sources_become_external_modules_with_credentials_stripped() {
    let out = TempDir::new("mod-remote");
    let raw = index(out.path());
    let graph: Value = serde_json::from_str(&raw).unwrap();
    let remote: Vec<_> = module_imports(&graph)
        .into_iter()
        .filter(|(_, _, _, ev)| ev[0] == "tf-module-remote")
        .collect();
    let targets: Vec<&str> = remote.iter().map(|(_, t, _, _)| t.as_str()).collect();
    assert_eq!(
        targets,
        vec![
            "git::https://example.com/org/net.git//modules/net",
            "terraform-aws-modules/vpc/aws",
        ]
    );
    assert!(remote.iter().all(|(_, _, c, _)| c == "certain"));
    // INV-6: neither the fake token nor the query string reached disk.
    for leaked in ["tok3n", "S3CR3T", "ci-user", "sshkey", "v1.2.0"] {
        assert!(!raw.contains(leaked), "graph.json leaks {leaked}");
    }
}

#[test]
fn module_output_and_arguments_cross_into_the_called_directory() {
    let out = TempDir::new("mod-cross");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    let refs = references(&graph);
    let has = |from: &str, to: &str, ev: &str| {
        refs.iter().any(|(f, t, c, e)| {
            f == from && t == to && c == "inferred" && e.contains(&ev.to_string())
        })
    };

    // `module.vpc.vpc_id` reaches modules/vpc's output, not the same-named
    // output declared in this very directory.
    let from = format!("output.vpc_id@{MODULES_TF}");
    assert!(has(
        &from,
        "output.vpc_id@infra/modules/vpc/outputs.tf",
        "tf-ref:module-output"
    ));
    assert!(
        !refs
            .iter()
            .any(|(f, t, _, _)| *f == from && t == &format!("output.vpc_id@{MODULES_TF}"))
    );

    // Arguments reach the target's variables.
    let module_vpc = format!("module.vpc@{MODULES_TF}");
    assert!(has(
        &module_vpc,
        "var.cidr@infra/modules/vpc/variables.tf",
        "tf-module-arg"
    ));
    assert!(has(
        &module_vpc,
        "var.region@infra/modules/vpc/variables.tf",
        "tf-module-arg"
    ));
    // ...and only vpc's, never app's.
    assert!(!refs.iter().any(|(f, t, _, e)| *f == module_vpc
        && t.contains("modules/app")
        && e.contains(&"tf-module-arg".to_string())));
    // The call's own inputs still reference the *caller's* variable.
    assert!(has(
        &module_vpc,
        &format!("var.region@{PROD}/variables.tf"),
        "tf-ref:same-module"
    ));
}

#[test]
fn unresolvable_module_pieces_are_reported_not_guessed() {
    let out = TempDir::new("mod-unresolved");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    let unresolved_of = |name: &str| -> Vec<String> {
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| {
                n["kind"] == "symbol"
                    && n["name"] == name
                    && n["file"] == graph_file_id(&graph, MODULES_TF)
            })
            .and_then(|n| n["unresolved_calls"].as_array())
            .map(|a| {
                a.iter()
                    .map(|u| u["name"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_else(|| panic!("no symbol {name}"))
    };
    assert_eq!(unresolved_of("module.vpc"), vec!["arg:bogus"]);
    assert_eq!(
        unresolved_of("module.missing_dir"),
        vec!["source:infra/modules/nope"]
    );
    assert_eq!(
        unresolved_of("output.bad_output"),
        vec!["module.vpc.nonexistent"]
    );
}

fn graph_file_id(graph: &Value, path: &str) -> String {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "file" && n["path"] == path)
        .and_then(|n| n["id"].as_str())
        .unwrap()
        .to_string()
}

#[test]
fn map_infra_section_summarizes_source_level_terraform_honestly() {
    let out = TempDir::new("map-infra");
    index(out.path());
    let stdout = Command::cargo_bin("carto")
        .unwrap()
        .args(["map", "--section", "infra"])
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(stdout).unwrap();
    assert!(text.contains("source-level Terraform"), "{text}");
    assert!(
        text.contains("infra/envs/prod -> infra/modules/vpc"),
        "{text}"
    );
    assert!(text.contains("terraform-aws-modules/vpc/aws"), "{text}");
    // Still honest that this is not the resolved infrastructure graph.
    assert!(text.contains("requires M2"), "{text}");
    assert!(text.contains("requires M3"), "{text}");
}

// ---- ADR-0044: a variable may be set outside the code ------------------

fn deps_stdout(out: &Path, args: &[&str]) -> String {
    let stdout = Command::cargo_bin("carto")
        .unwrap()
        .arg("deps")
        .args(args)
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

#[test]
fn tfvars_files_are_recorded_as_sensitive_and_never_hashed() {
    let out = TempDir::new("tfvars-walk");
    let graph: Value = serde_json::from_str(&index(out.path())).unwrap();
    for path in [
        "infra/envs/prod/prod.tfvars",
        "infra/envs/prod/prod.auto.tfvars.json",
    ] {
        let node = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["kind"] == "file" && n["path"] == path)
            .unwrap_or_else(|| panic!("no File node for {path}"));
        assert_eq!(node["excluded"], "sensitive", "{path}");
        assert!(node["sha256"].is_null(), "{path} must never be hashed");
    }
}

#[test]
fn deps_on_a_variable_names_the_tfvars_files_beside_it_but_never_their_contents() {
    let out = TempDir::new("tfvars-note");
    index(out.path());
    let text = deps_stdout(
        out.path(),
        &["var.region", "--dir", "in", "--subpath", "infra/envs/prod"],
    );
    assert!(text.contains("infra/envs/prod/prod.tfvars"), "{text}");
    assert!(
        text.contains("infra/envs/prod/prod.auto.tfvars.json"),
        "{text}"
    );
    assert!(text.contains("contents never read"), "{text}");
    assert!(text.contains("TF_VAR_*"), "{text}");
    assert!(
        text.contains("not evidence it is unset or unused"),
        "{text}"
    );
    assert!(!text.contains("CANARY"), "{text}");
}

#[test]
fn the_caveat_is_always_printed_for_a_variable_even_with_no_tfvars_file() {
    let out = TempDir::new("tfvars-none");
    index(out.path());
    let text = deps_stdout(
        out.path(),
        &[
            "var.region",
            "--dir",
            "in",
            "--subpath",
            "infra/modules/vpc",
        ],
    );
    assert!(
        text.contains("no *.tfvars file in this directory"),
        "{text}"
    );
    assert!(
        text.contains("not evidence it is unset or unused"),
        "{text}"
    );
}

#[test]
fn the_caveat_is_inbound_only_and_variable_only() {
    let out = TempDir::new("tfvars-scope");
    index(out.path());
    // Outbound question about the same variable: no caveat.
    let outbound = deps_stdout(
        out.path(),
        &["var.region", "--dir", "out", "--subpath", "infra/envs/prod"],
    );
    assert!(!outbound.contains("TF_VAR_"), "{outbound}");
    // Not a variable: no caveat, even inbound.
    let resource = deps_stdout(out.path(), &["aws_vpc.main", "--dir", "in"]);
    assert!(!resource.contains("TF_VAR_"), "{resource}");
}

#[test]
fn json_carries_the_structured_field_only_for_variable_roots() {
    let out = TempDir::new("tfvars-json");
    index(out.path());
    let run = |args: &[&str]| -> Value {
        serde_json::from_str(&deps_stdout(out.path(), &[args, &["--json"]].concat())).unwrap()
    };
    let var = run(&["var.region", "--dir", "in", "--subpath", "infra/envs/prod"]);
    assert_eq!(
        var["root_may_be_set_externally"]["tfvars_files"],
        serde_json::json!([
            "infra/envs/prod/prod.auto.tfvars.json",
            "infra/envs/prod/prod.tfvars"
        ])
    );
    let module_var = run(&[
        "var.region",
        "--dir",
        "in",
        "--subpath",
        "infra/modules/vpc",
    ]);
    assert_eq!(
        module_var["root_may_be_set_externally"]["tfvars_files"],
        serde_json::json!([])
    );
    let resource = run(&["aws_vpc.main", "--dir", "in"]);
    assert!(resource["root_may_be_set_externally"].is_null());
}

#[test]
fn map_infra_says_when_terraform_exists_but_not_in_scope() {
    // `fixtures/monorepo` has Terraform under `infra/` and Go/C#/TS/Python
    // elsewhere: a scope without Terraform must not read as "no infra".
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/monorepo");
    let out = TempDir::new("map-scope");
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(&repo)
        .arg("--out")
        .arg(out.path())
        .assert()
        .success();
    let map = |subpath: &str| -> String {
        let stdout = Command::cargo_bin("carto")
            .unwrap()
            .args(["map", "--section", "infra", "--subpath", subpath])
            .arg(&repo)
            .arg("--out")
            .arg(out.path())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(stdout).unwrap()
    };
    let outside = map("services");
    assert!(outside.contains("no Terraform in this scope"), "{outside}");
    assert!(
        !outside.contains("none — requires M2 (infrastructure graph)"),
        "{outside}"
    );
    let inside = map("infra");
    assert!(inside.contains("source-level Terraform"), "{inside}");
}
