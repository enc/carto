//! CLI-level integration tests, run via `cargo test --workspace`
//! (`scripts/gates.sh`). These close out the two spec §9.5 gates
//! ADR-0004 deferred pending an `index` command that actually writes
//! through `PathGuard`:
//!
//! - gate 3, `test_determinism`: double-index, byte-compare `graph.json`
//!   (INV-7).
//! - gate 4, `test_pathguard`: `--out` into the INV-4 denylist exits 3.
//!
//! Plus a golden-file check against `fixtures/mixed.graph.golden.json`.

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mixed")
}

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mixed.graph.golden.json")
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "carto-cli-test-{tag}-{}-{:?}",
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

fn run_index(repo: &Path, out: &Path, extra_args: &[&str]) -> assert_cmd::assert::Assert {
    Command::cargo_bin("carto")
        .unwrap()
        .arg("index")
        .arg(repo)
        .arg("--out")
        .arg(out)
        .args(extra_args)
        .assert()
}

/// Spec §9.5 gate 3 (INV-7): the same input tree indexed twice, into two
/// independent out-dirs, must produce byte-identical `graph.json`.
#[test]
fn index_produces_byte_identical_graph_json_across_two_runs() {
    let out_a = TempDir::new("det-a");
    let out_b = TempDir::new("det-b");

    run_index(&fixture_path(), out_a.path(), &[]).success();
    run_index(&fixture_path(), out_b.path(), &[]).success();

    let a = std::fs::read(out_a.path().join("graph.json")).unwrap();
    let b = std::fs::read(out_b.path().join("graph.json")).unwrap();
    assert_eq!(
        a, b,
        "graph.json must be byte-identical across independent runs (INV-7)"
    );
}

/// Spec §9.5 gate 4: a write that resolves onto the INV-4 denylist is
/// refused with exit code 3, and creates nothing (not even the directory
/// component leading up to the refusal — pathguard's `new()` checks the
/// denylist before `create_dir_all`).
#[test]
fn index_refuses_a_denylisted_out_dir_with_exit_code_3() {
    let base = TempDir::new("pathguard");
    let denylisted_out = base.path().join(".claude").join("carto-out");

    run_index(&fixture_path(), &denylisted_out, &[])
        .failure()
        .code(3);

    assert!(!denylisted_out.exists());
    assert!(!base.path().join(".claude").exists());
}

/// Spec §9.2: `--json` stdout is pure data — nothing else may be mixed
/// into it.
#[test]
fn index_json_stdout_is_pure_json() {
    let out = TempDir::new("json");
    let output = Command::cargo_bin("carto")
        .unwrap()
        .args(["index"])
        .arg(fixture_path())
        .arg("--out")
        .arg(out.path())
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout was not pure JSON: {e}\nstdout was: {stdout:?}"));
    assert!(parsed.get("node_count").is_some());
}

/// INV-3: indexing must never modify the repository it reads.
#[test]
fn index_repo_root_is_left_untouched() {
    let out = TempDir::new("readonly");
    let before = walk_paths(&fixture_path());

    run_index(&fixture_path(), out.path(), &[]).success();

    let after = walk_paths(&fixture_path());
    assert_eq!(
        before, after,
        "indexing must never modify the repo it reads (INV-3)"
    );
}

fn walk_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

/// Golden-file check: `fixtures/mixed` must index to exactly
/// `fixtures/mixed.graph.golden.json`, modulo `carto_version` (a version
/// bump alone shouldn't force a golden-file rewrite — separately asserted
/// against the crate's real version below). Regenerate with:
/// `CARTO_UPDATE_GOLDEN=1 cargo test -p carto-cli --test cli index_matches_golden_graph_json`
#[test]
fn index_matches_golden_graph_json() {
    let out = TempDir::new("golden");
    run_index(&fixture_path(), out.path(), &[]).success();

    let actual = std::fs::read_to_string(out.path().join("graph.json")).unwrap();
    let normalized = normalize_carto_version(&actual);

    if std::env::var_os("CARTO_UPDATE_GOLDEN").is_some() {
        std::fs::write(golden_path(), &normalized).unwrap();
        return;
    }

    let golden = std::fs::read_to_string(golden_path()).unwrap_or_else(|e| {
        panic!(
            "failed to read golden file {}: {e}\nrun with CARTO_UPDATE_GOLDEN=1 to create it",
            golden_path().display()
        )
    });
    assert_eq!(
        normalized, golden,
        "graph.json for fixtures/mixed no longer matches fixtures/mixed.graph.golden.json.\n\
         If this change is expected, regenerate with:\n  \
         CARTO_UPDATE_GOLDEN=1 cargo test -p carto-cli --test cli index_matches_golden_graph_json"
    );
}

/// Replaces `carto_version`'s value with a placeholder so a version bump
/// alone doesn't force a golden-file rewrite, after confirming the real
/// `graph.json` did report the crate's actual version.
fn normalize_carto_version(graph_json: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(graph_json).unwrap();
    let version = value
        .get("carto_version")
        .and_then(|v| v.as_str())
        .expect("graph.json must have a carto_version field")
        .to_string();
    assert_eq!(
        version,
        env!("CARGO_PKG_VERSION"),
        "graph.json's carto_version should be the crate's own version"
    );

    let needle = format!("\"carto_version\": \"{version}\"");
    assert!(
        graph_json.contains(&needle),
        "expected to find {needle:?} in graph.json for normalization"
    );
    graph_json.replacen(&needle, "\"carto_version\": \"<carto_version>\"", 1)
}
