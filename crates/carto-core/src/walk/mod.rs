//! Repo traversal (spec §5.1). Produces `File` nodes only — no parsing
//! (that's `lang`'s job, from M1.b.2 onward). Uses the `ignore` crate's
//! own parallelism; results are funneled through a channel to a single
//! collector, sorted, and returned — never inserted into the graph in
//! whatever order threads happen to finish in (determinism, INV-7).

use crate::consts::MAX_FILE_BYTES;
use crate::error::{Error, ErrorKind, Result};
use crate::graph::{self, ExclusionReason, FileNode, Node, SkipReason};
use crate::lang::Lang;
use crate::taint::Provenance;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::mpsc;

/// Custom ignore file carto respects in addition to the `.gitignore`
/// family (spec §5.1: "same syntax, additive").
const CARTOIGNORE_FILENAME: &str = ".cartoignore";

/// Directory-name components pruned from the walk entirely — no `File`
/// node, no descent — regardless of `--no-gitignore` (spec §5.1).
const DENIED_DIR_NAMES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];

/// Every pattern string from spec §5.1's built-in denylist, gathered in
/// one place purely so [`ignore_rule_digest`] can hash "the built-in
/// denylist" as a single stable value. Not used for matching (the actual
/// matching logic below is hand-written per pattern, since these are
/// simple enough not to need a glob engine).
const DENYLIST_DESCRIPTION: &[&str] = &[
    ".git/",
    "node_modules/",
    "target/",
    "dist/",
    "build/",
    ".venv/",
    "venv/",
    "__pycache__/",
    "*.min.js",
    "*.lock",
    "*.pem",
    "*.key",
    "*.p12",
    ".env*",
    "*credentials*",
    "*.tfstate*",
    "*.tfvars",
];

/// Result of [`walk`]: `File` nodes ready to insert into a [`graph::Graph`],
/// already sorted by repo-relative path (spec §5.1: "collect, then sort,
/// then insert"), plus the bits [`crate::graph::persist::Manifest`] needs.
pub struct WalkOutput {
    pub nodes: Vec<Node>,
    /// repo-relative path -> sha256 hex, for every file whose contents
    /// were actually read (mirrors each such `FileNode.sha256`).
    pub file_sha256: BTreeMap<String, String>,
    pub ignore_rule_digest: String,
}

/// Walks `repo_root` (spec §5.1) and returns every eligible file as a
/// `File` node. `respect_gitignore = false` corresponds to the CLI's
/// `--no-gitignore`: it disables `.gitignore`/`.git/info/exclude`/global
/// gitignore, but never the built-in denylist above, and never
/// `.cartoignore` (carto's own ignore file, additive and independent of
/// git).
pub fn walk(repo_root: &Path, respect_gitignore: bool) -> Result<WalkOutput> {
    let root = repo_root.canonicalize().map_err(|e| {
        Error::with_source(
            ErrorKind::UserError,
            format!("cannot resolve repo path `{}`", repo_root.display()),
            e,
        )
    })?;

    let ignore_rule_digest = ignore_rule_digest(&root, respect_gitignore)?;

    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .parents(respect_gitignore)
        .require_git(false)
        .add_custom_ignore_filename(CARTOIGNORE_FILENAME)
        .filter_entry(|entry| {
            !(entry.file_type().is_some_and(|t| t.is_dir())
                && is_denied_dir_name(entry.file_name().to_str().unwrap_or("")))
        });

    let (tx, rx) = mpsc::channel::<WalkedFile>();
    builder.build_parallel().run(|| {
        let tx = tx.clone();
        let root = root.clone();
        Box::new(move |result| {
            if let Ok(entry) = result {
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    if let Some(walked) = classify(&root, entry.path()) {
                        // A closed receiver (collector panicked) shouldn't
                        // take down worker threads; best-effort send.
                        let _ = tx.send(walked);
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });
    drop(tx);

    let mut collected: Vec<WalkedFile> = rx.into_iter().collect();
    collected.sort_by(|a, b| path_of(&a.node).cmp(path_of(&b.node)));

    let mut file_sha256 = BTreeMap::new();
    let mut nodes = Vec::with_capacity(collected.len());
    for walked in collected {
        if let Some((path, sha)) = walked.hashed {
            file_sha256.insert(path, sha);
        }
        nodes.push(walked.node);
    }

    Ok(WalkOutput {
        nodes,
        file_sha256,
        ignore_rule_digest,
    })
}

struct WalkedFile {
    node: Node,
    hashed: Option<(String, String)>,
}

/// [`Lang::from_extension`] only ever sees a file name's last extension
/// segment (`Path::extension()`'s own contract). A real Terraform
/// monorepo pattern this misses: `locals.tf.simu`/`locals.tf.prod`, a
/// per-environment source file (ADR-0025) whose contents `prepare-
/// environment.sh`-style tooling copies into a gitignored `locals_env.
/// tf` — `Path::extension()` alone would see `simu`/`prod` and classify
/// it `Other`, never reaching the HCL extractor. Checked first, ahead of
/// the ordinary single-extension lookup; general (`<name>.tf.<anything>`
/// → `Hcl`), not a `simu`/`prod` allowlist.
fn lang_for_file_name(file_name: &str) -> Lang {
    let parts: Vec<&str> = file_name.rsplitn(3, '.').collect();
    if parts.len() == 3 && parts[1].eq_ignore_ascii_case("tf") {
        return Lang::Hcl;
    }
    Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(Lang::from_extension)
        .unwrap_or(Lang::Other)
}

fn path_of(node: &Node) -> &str {
    &node
        .data
        .as_file()
        .expect("walk only ever constructs File nodes")
        .path
}

/// Reads and classifies a single file at `abs_path` (already known to be
/// a regular file), returning `None` only if the path can't be made
/// repo-relative (shouldn't happen — `abs_path` always comes from a walk
/// rooted at `root`).
fn classify(root: &Path, abs_path: &Path) -> Option<WalkedFile> {
    let rel = abs_path.strip_prefix(root).ok()?;
    let path = to_repo_relative_string(rel);
    let file_name = abs_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let lang = lang_for_file_name(file_name);

    if is_sensitive(file_name) {
        // INV-3/§5.1: contents never read. `size` still comes from
        // metadata (a stat, not a content read).
        let size = std::fs::metadata(abs_path).map(|m| m.len()).unwrap_or(0);
        let file = FileNode {
            path: path.clone(),
            lang,
            size,
            sha256: None,
            skipped: None,
            excluded: Some(ExclusionReason::Sensitive),
            component: None,
        };
        return Some(WalkedFile {
            node: Node::file(graph::file_id(&path), Provenance::Syntactic, "walk@1", file),
            hashed: None,
        });
    }

    let metadata_len = std::fs::metadata(abs_path).map(|m| m.len()).unwrap_or(0);
    if metadata_len > MAX_FILE_BYTES {
        let file = FileNode {
            path: path.clone(),
            lang,
            size: metadata_len,
            sha256: None,
            skipped: Some(SkipReason::TooLarge),
            excluded: None,
            component: None,
        };
        return Some(WalkedFile {
            node: Node::file(graph::file_id(&path), Provenance::Syntactic, "walk@1", file),
            hashed: None,
        });
    }

    // Read once: covers both the binary sniff (first 8 KiB) and the
    // sha256 digest (whole content). A best-effort read — a file that
    // vanishes or becomes unreadable between the metadata stat above and
    // here (TOCTOU) is dropped from the graph rather than failing the
    // whole walk.
    let content = std::fs::read(abs_path).ok()?;
    let size = content.len() as u64;
    let sniff_len = content.len().min(8192);
    let is_binary = content[..sniff_len].contains(&0u8);

    let skipped = if is_binary {
        Some(SkipReason::Binary)
    } else if file_name.ends_with(".lock") {
        Some(SkipReason::LockFile)
    } else if file_name.ends_with(".min.js") {
        Some(SkipReason::Minified)
    } else {
        None
    };

    let sha256_hex = to_hex(&Sha256::digest(&content));
    let file = FileNode {
        path: path.clone(),
        lang,
        size,
        sha256: Some(sha256_hex.clone()),
        skipped,
        excluded: None,
        component: None,
    };
    Some(WalkedFile {
        node: Node::file(graph::file_id(&path), Provenance::Syntactic, "walk@1", file),
        hashed: Some((path, sha256_hex)),
    })
}

fn is_denied_dir_name(name: &str) -> bool {
    DENIED_DIR_NAMES.contains(&name)
}

/// Spec §5.1's secret-shaped set: `*.pem`, `*.key`, `*.p12`, `.env*`,
/// `*credentials*`, `*.tfstate*`, `*.tfvars`.
fn is_sensitive(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.starts_with(".env")
        || lower.contains("credentials")
        || lower.contains(".tfstate")
        || lower.ends_with(".tfvars")
}

/// Converts a path (relative, from `strip_prefix`) into the repo-relative,
/// always-`/`-separated string spec §4.1 requires, regardless of host OS
/// path separator.
fn to_repo_relative_string(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").expect("writing to a String never fails");
    }
    s
}

/// A stable digest of "the ignore rules currently in effect": the
/// built-in denylist (spec §5.1) plus the content of every `.cartoignore`
/// found under `root` plus whether `.gitignore`-family respecting is on.
/// Recorded in the manifest (spec §4.4) so an operator can tell whether
/// two indexes were built under the same ignore configuration.
fn ignore_rule_digest(root: &Path, respect_gitignore: bool) -> Result<String> {
    let mut input = Vec::new();
    for pattern in DENYLIST_DESCRIPTION {
        input.extend_from_slice(pattern.as_bytes());
        input.push(0);
    }
    for (path, bytes) in find_cartoignore_files(root)? {
        input.extend_from_slice(path.as_bytes());
        input.extend_from_slice(&bytes);
    }
    input.push(u8::from(respect_gitignore));
    Ok(graph::id::blake3_hex_prefix(&input))
}

/// Sequential scan (determinism matters more than speed here; there are
/// normally very few `.cartoignore` files) for every `.cartoignore` under
/// `root`, skipping the same built-in denied directories as the main walk
/// so a stray one inside e.g. `node_modules/` isn't picked up. Independent
/// of `respect_gitignore` and of `.gitignore` itself — `.cartoignore` is
/// carto's own file, not git's.
fn find_cartoignore_files(root: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .require_git(false)
        .filter_entry(|entry| {
            !(entry.file_type().is_some_and(|t| t.is_dir())
                && is_denied_dir_name(entry.file_name().to_str().unwrap_or("")))
        });

    let mut out = Vec::new();
    for entry in builder.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().is_some_and(|t| t.is_file())
            && entry.file_name().to_str() == Some(CARTOIGNORE_FILENAME)
        {
            let rel = entry
                .path()
                .strip_prefix(root)
                .expect("entry is always under root");
            let path = to_repo_relative_string(rel);
            let bytes = std::fs::read(entry.path())?;
            out.push((path, bytes));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "carto-walk-test-{tag}-{}-{:?}",
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

    fn write(dir: &Path, rel: &str, content: &[u8]) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn node_path<'a>(out: &'a WalkOutput, path: &str) -> Option<&'a FileNode> {
        out.nodes.iter().find_map(|n| {
            let f = n.data.as_file().unwrap();
            (f.path == path).then_some(f)
        })
    }

    #[test]
    fn denylisted_directories_produce_no_nodes_and_are_not_descended() {
        let dir = TempDir::new("denylist-dirs");
        write(dir.path(), "src/main.rs", b"fn main() {}");
        write(
            dir.path(),
            "node_modules/pkg/index.js",
            b"module.exports = {}",
        );
        write(dir.path(), "target/debug/build.log", b"log");
        write(dir.path(), ".git/HEAD", b"ref: refs/heads/main");

        let out = walk(dir.path(), true).unwrap();
        assert!(node_path(&out, "src/main.rs").is_some());
        assert!(node_path(&out, "node_modules/pkg/index.js").is_none());
        assert!(node_path(&out, "target/debug/build.log").is_none());
        assert!(node_path(&out, ".git/HEAD").is_none());
    }

    #[test]
    fn sensitive_files_are_excluded_and_never_read() {
        let dir = TempDir::new("sensitive");
        write(dir.path(), ".env", b"SECRET=abc123");
        write(dir.path(), "fake.pem", b"-----BEGIN FAKE KEY-----");
        write(dir.path(), "aws-credentials.json", b"{\"key\":\"fake\"}");
        write(dir.path(), "prod.tfvars", b"password = \"fake\"");

        let out = walk(dir.path(), true).unwrap();
        for path in [".env", "fake.pem", "aws-credentials.json", "prod.tfvars"] {
            let node = node_path(&out, path).unwrap_or_else(|| panic!("missing node for {path}"));
            assert_eq!(node.excluded, Some(ExclusionReason::Sensitive));
            assert_eq!(node.sha256, None, "{path} contents must never be hashed");
        }
    }

    #[test]
    fn lock_and_minified_files_are_recorded_but_marked_never_parsed() {
        let dir = TempDir::new("lockmin");
        write(dir.path(), "package-lock.lock", b"{}");
        write(dir.path(), "vendor.min.js", b"!function(){}();");

        let out = walk(dir.path(), true).unwrap();
        assert_eq!(
            node_path(&out, "package-lock.lock").unwrap().skipped,
            Some(SkipReason::LockFile)
        );
        assert_eq!(
            node_path(&out, "vendor.min.js").unwrap().skipped,
            Some(SkipReason::Minified)
        );
    }

    #[test]
    fn binary_files_are_skipped_but_still_hashed() {
        let dir = TempDir::new("binary");
        write(dir.path(), "photo.bin", &[0u8, 1, 2, 3, 0, 4]);

        let out = walk(dir.path(), true).unwrap();
        let node = node_path(&out, "photo.bin").unwrap();
        assert_eq!(node.skipped, Some(SkipReason::Binary));
        assert!(node.sha256.is_some());
    }

    #[test]
    fn oversized_files_are_skipped_without_reading_content() {
        let dir = TempDir::new("toolarge");
        let big = vec![b'a'; (MAX_FILE_BYTES + 1) as usize];
        write(dir.path(), "huge.txt", &big);

        let out = walk(dir.path(), true).unwrap();
        let node = node_path(&out, "huge.txt").unwrap();
        assert_eq!(node.skipped, Some(SkipReason::TooLarge));
        assert_eq!(node.sha256, None);
    }

    #[test]
    fn cartoignore_excludes_paths_additively() {
        let dir = TempDir::new("cartoignore");
        write(dir.path(), "keep.rs", b"fn keep() {}");
        write(dir.path(), "generated/skip.rs", b"fn skip() {}");
        write(dir.path(), ".cartoignore", b"generated/\n");

        let out = walk(dir.path(), true).unwrap();
        assert!(node_path(&out, "keep.rs").is_some());
        assert!(node_path(&out, "generated/skip.rs").is_none());
        // The digest must reflect the .cartoignore's presence/content.
        let out_without = {
            std::fs::remove_file(dir.path().join(".cartoignore")).unwrap();
            walk(dir.path(), true).unwrap()
        };
        assert_ne!(out.ignore_rule_digest, out_without.ignore_rule_digest);
    }

    #[test]
    fn gitignore_is_respected_unless_disabled() {
        let dir = TempDir::new("gitignore");
        write(dir.path(), "kept.rs", b"fn kept() {}");
        write(dir.path(), "ignored.rs", b"fn ignored() {}");
        write(dir.path(), ".gitignore", b"ignored.rs\n");

        let respected = walk(dir.path(), true).unwrap();
        assert!(node_path(&respected, "kept.rs").is_some());
        assert!(node_path(&respected, "ignored.rs").is_none());

        let disabled = walk(dir.path(), false).unwrap();
        assert!(node_path(&disabled, "ignored.rs").is_some());
    }

    #[test]
    fn results_are_sorted_by_path_regardless_of_filesystem_order() {
        let dir = TempDir::new("sorted");
        write(dir.path(), "zzz.rs", b"");
        write(dir.path(), "aaa.rs", b"");
        write(dir.path(), "mmm.rs", b"");

        let out = walk(dir.path(), true).unwrap();
        let paths: Vec<&str> = out
            .nodes
            .iter()
            .map(|n| n.data.as_file().unwrap().path.as_str())
            .collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
    }

    #[test]
    fn two_part_tf_extension_classifies_as_hcl() {
        let dir = TempDir::new("tf-two-part-ext");
        write(dir.path(), "locals.tf.simu", b"locals { x = 1 }");
        write(dir.path(), "locals.tf.prod", b"locals { x = 2 }");
        write(dir.path(), "main.tf", b"");
        write(dir.path(), "notes.txt.bak", b"");

        let out = walk(dir.path(), true).unwrap();
        assert_eq!(node_path(&out, "locals.tf.simu").unwrap().lang, Lang::Hcl);
        assert_eq!(node_path(&out, "locals.tf.prod").unwrap().lang, Lang::Hcl);
        assert_eq!(node_path(&out, "main.tf").unwrap().lang, Lang::Hcl);
        assert_eq!(node_path(&out, "notes.txt.bak").unwrap().lang, Lang::Other);
    }
}
