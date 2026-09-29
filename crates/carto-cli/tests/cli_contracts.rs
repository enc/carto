//! CLI-level acceptance test for ADR-0026's cross-language contract
//! slice, against `fixtures/sid-like`. Exercises the actual acceptance
//! test named in `docs/adr/0026-contract-node-and-produces-consumes-
//! edges.md`: "which Terraform CloudWatch metric names are never
//! emitted by any service?"

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sid-like")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-contracts-test-{tag}-{}-{:?}",
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

fn index(out: &Path) {
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(fixture_path())
        .arg("--out")
        .arg(out)
        .assert()
        .success();
}

#[test]
fn orphans_finds_exactly_the_dead_alarm_and_the_unwatched_metric() {
    let tmp = TempDir::new("orphans");
    index(tmp.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .args(["orphans", "--category", "metric_name", "--json"])
        .arg(fixture_path())
        .arg("--out")
        .arg(tmp.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();

    let consumed_never_produced: Vec<String> = json["consumed_never_produced"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap().to_string())
        .collect();
    let produced_never_consumed: Vec<String> = json["produced_never_consumed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(
        consumed_never_produced.len(),
        2,
        "expected exactly the dead alarm plus the cross-namespace orphan, got {consumed_never_produced:?}"
    );
    assert!(consumed_never_produced.contains(&"SequenceGapsTotal".to_string()));
    assert!(consumed_never_produced.contains(&"KafkaErrorsTotal".to_string()));

    assert_eq!(
        produced_never_consumed,
        vec!["KafkaErrorsTotal".to_string()]
    );

    // The matched pair and the interpolated alarm must appear in
    // neither list.
    let mut all = consumed_never_produced.clone();
    all.extend(produced_never_consumed.clone());
    assert!(!all.contains(&"QuoteDropsPerSecond".to_string()));
}

#[test]
fn contract_lookup_shows_both_sides_of_the_matched_pair() {
    let tmp = TempDir::new("contract");
    index(tmp.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .args(["contract", "QuoteDropsPerSecond", "--json"])
        .arg(fixture_path())
        .arg("--out")
        .arg(tmp.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();

    let matches = json["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["producers"].as_array().unwrap().len(), 1);
    assert_eq!(matches[0]["consumers"].as_array().unwrap().len(), 1);
}

#[test]
fn two_part_tf_extension_file_is_indexed_as_hcl_with_no_crash() {
    let tmp = TempDir::new("two-part-ext");
    index(tmp.path());

    let graph_json = std::fs::read_to_string(tmp.path().join("graph.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&graph_json).unwrap();
    let locals_node = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["path"] == "infra/locals.tf.simu")
        .expect("locals.tf.simu must be indexed as a File node");
    assert_eq!(locals_node["lang"], "hcl");
}

/// ADR-0041: an HCL contract literal now attaches to the enclosing
/// resource symbol (the alarm), not just to its `File` node.
#[test]
fn hcl_consumer_site_is_the_alarm_resource_symbol() {
    let tmp = TempDir::new("hcl-site");
    index(tmp.path());

    let output = Command::cargo_bin("carto")
        .unwrap()
        .args(["contract", "SequenceGapsTotal"])
        .arg(fixture_path())
        .arg("--out")
        .arg(tmp.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains("aws_cloudwatch_metric_alarm.sequence_gaps"),
        "{text}"
    );
}
