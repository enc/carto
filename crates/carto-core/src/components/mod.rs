//! Component discovery — multi-root support (not spec vocabulary; see
//! `docs/adr/0034-component-discovery-and-scoping.md`). A real repo
//! often puts several projects that belong together under one walked
//! tree: microservices, frontends, lambdas, and the infrastructure code
//! that deploys them. A [`Component`] is a directory inside that tree
//! that is the root of one such project; every walked file belongs to
//! at most one component — the *innermost* component whose path is a
//! prefix of the file's own path (`component_of_path`'s docs have the
//! exact rule). A file under no component root is a real, meaningful
//! bucket (`None`), not a missing value.
//!
//! Discovery runs over `walk`'s own output — no second traversal, and
//! `.gitignore`/the built-in denylist/`.cartoignore` are respected for
//! free, the same reuse `resolve.rs` gets from being handed `walk`'s
//! `File` nodes rather than re-reading the tree itself. Auto-detection
//! (manifest-file markers) can be overridden or extended by an optional
//! repo-local `.carto/roots.json`, following the exact built-ins-plus-
//! repo-file shape `crate::contracts::ContractRules` already
//! established (ADR-0027): JSON (no new dependency), closed-vocabulary
//! fields hard-error naming the file and the bad value, `kind` stays
//! open.

use crate::error::{Error, ErrorKind, Result};
use crate::graph::Node;
use crate::lang::Lang;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Repo-relative path of the optional root-declaration override file.
const CONFIG_RELPATH: &str = ".carto/roots.json";

/// One discovered or declared project root inside the walked tree.
/// `Serialize`/`Deserialize`: persisted verbatim as `GraphDocument::
/// components` (sorted by `path`, INV-7) — this is the on-disk shape,
/// not an internal detail either front end reshapes freely, the same
/// compatibility-surface status every other `graph.json`-visible type
/// already has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    /// Sanitized to `^[A-Za-z0-9][A-Za-z0-9_.-]*$` — a plain `String`,
    /// not `TaintedString`: like `FileNode::path`, this is an
    /// extractor-computed identifier (a directory basename, or a
    /// repo-author-declared config value validated against a closed
    /// charset), never captured source text (INV-5 has nothing to say
    /// about it, the same reasoning `where_cmd.rs` already applies to
    /// `FileNode::path`).
    pub name: String,
    /// repo-relative, `/`-separated, no trailing slash — matches
    /// `FileNode::path`'s own convention exactly.
    pub path: String,
    /// Free-form (open vocabulary, unlike `name`'s charset or a
    /// `.carto/contracts.json` rule's `role`/`lang`): `"go"`, `"node"`,
    /// `"rust"`, `"python"`, `"php"`, `"dotnet"`, `"terraform"` for an
    /// auto-detected component; whatever a declared root's own `kind`
    /// says (default `"custom"` when omitted).
    pub kind: String,
}

/// The full set of components discovered/declared for one index build.
#[derive(Debug)]
pub struct ComponentSet {
    /// Sorted by `path` descending (deepest first), so
    /// [`ComponentSet::component_of_path`]'s linear scan finds the
    /// innermost match first — the same "longest prefix wins" shape
    /// `QueryGraph::path_in_scope` already uses for subpath scoping,
    /// just applied to pick one component rather than a yes/no filter.
    components: Vec<Component>,
    /// blake3 hex digest of the config file's presence/content —
    /// closes, for `.carto/roots.json`, the same provenance gap
    /// ADR-0027 left open for `.carto/contracts.json` (STATUS.md's
    /// "deliberately absent" list). Always present (a missing file has
    /// a canonical digest too), mirroring `walk::ignore_rule_digest`'s
    /// own shape.
    config_digest: String,
}

impl ComponentSet {
    /// No components at all — the pre-multi-root state, and what every
    /// existing single-project fixture/repo still gets today (no
    /// `.carto/roots.json`, no directory anywhere under a walked
    /// non-root subdirectory carrying a recognized manifest marker).
    pub fn empty() -> Self {
        ComponentSet {
            components: Vec::new(),
            config_digest: config_digest_for(None),
        }
    }

    pub fn components(&self) -> &[Component] {
        &self.components
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    pub fn config_digest(&self) -> &str {
        &self.config_digest
    }

    /// Components sorted by `path` ascending — the shape persisted into
    /// `GraphDocument::components` (INV-7: on-disk order must be a pure
    /// function of content; `path` is each component's own natural sort
    /// key, the same role `id` plays for nodes/edges elsewhere in
    /// `graph.json`). Deliberately not the order `components` is stored
    /// in internally (deepest-first, for `component_of_path`'s lookup).
    pub fn components_sorted_by_path(&self) -> Vec<Component> {
        let mut v = self.components.clone();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    /// The innermost component whose `path` is a prefix of `file_path`
    /// at a segment boundary (`file_path == c.path` or
    /// `file_path.starts_with("{c.path}/")` — identical rule to
    /// `QueryGraph::path_in_scope`, so `"services/orders"` doesn't also
    /// match `"services/orders2/x.go"`). `components` is sorted deepest
    /// path first, so the first match is always the innermost —
    /// component paths never overlap two ways (one is always either a
    /// strict ancestor of the other or disjoint; see
    /// [`ComponentSet::discover`]'s dedup pass), so "innermost" and
    /// "first match" are the same thing here.
    pub fn component_of_path(&self, file_path: &str) -> Option<&str> {
        self.components
            .iter()
            .find(|c| {
                file_path == c.path.as_str() || file_path.starts_with(&format!("{}/", c.path))
            })
            .map(|c| c.name.as_str())
    }

    /// Auto-detects components from `files` (expected: `walk`'s own
    /// `WalkOutput::nodes`, `File` nodes only — anything else is
    /// ignored) by manifest-file markers, then layers
    /// `<repo_root>/.carto/roots.json` on top if present. See the
    /// module doc for the marker table and the aggregator/collision
    /// rules below.
    pub fn discover(repo_root: &Path, files: &[Node]) -> Result<Self> {
        let config = load_config(repo_root)?;
        let should_detect = config
            .as_ref()
            .and_then(|c| c.detect)
            .unwrap_or_else(|| config.as_ref().is_none_or(|c| c.roots.is_empty()));

        let mut detected = if should_detect {
            detect_components(repo_root, files)?
        } else {
            Vec::new()
        };

        if let Some(config) = &config {
            apply_declared_roots(repo_root, files, config, &mut detected)?;
        }

        detected.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.path.cmp(&b.path)));

        Ok(ComponentSet {
            components: detected,
            config_digest: config_digest_for(read_config_bytes(repo_root)?.as_deref()),
        })
    }
}

/// Every directory that contains at least one walked file, plus what
/// carto needs to decide a marker: the set of basenames present
/// directly in it, and whether any of those files is HCL (the
/// Terraform marker is "at least one `.tf`/`.tf.<env>` file", not a
/// single named manifest — ADR-0025's two-part-extension fix already
/// makes `Lang::Hcl` the right signal for both shapes).
#[derive(Default)]
struct DirInfo<'a> {
    basenames: Vec<&'a str>,
    has_hcl: bool,
}

fn dir_and_basename(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((dir, base)) => (dir, base),
        None => ("", path),
    }
}

fn collect_dirs(files: &[Node]) -> BTreeMap<&str, DirInfo<'_>> {
    let mut dirs: BTreeMap<&str, DirInfo<'_>> = BTreeMap::new();
    for node in files {
        let Some(file) = node.data.as_file() else {
            continue;
        };
        let (dir, basename) = dir_and_basename(file.path.as_str());
        let info = dirs.entry(dir).or_default();
        info.basenames.push(basename);
        if file.lang == Lang::Hcl {
            info.has_hcl = true;
        }
    }
    dirs
}

/// Priority-ordered manifest markers, checked in this order per
/// directory — first match wins, same discipline `resolve.rs`'s tier
/// ladder and `.carto/contracts.json`'s rule scan both already use.
/// Terraform is last: "at least one `.tf` file" is a much weaker signal
/// than an explicit package manifest naming exactly one project.
const EXACT_MARKERS: &[(&str, &str)] = &[
    ("go.mod", "go"),
    ("package.json", "node"),
    ("Cargo.toml", "rust"),
    ("pyproject.toml", "python"),
    ("setup.py", "python"),
    ("composer.json", "php"),
];

/// Auto-detects one candidate component per qualifying directory. The
/// walked root itself (`dir == ""`) is never a candidate, deliberately
/// — a component's whole point is to name a project *nested inside*
/// the tree, distinguishable from siblings; a marker at the root
/// describes the repo as a whole, which is already the "no narrower
/// scope" (`component: None`) case every single-project repo has today.
/// This is also what keeps every pre-existing fixture's own top-level
/// `Cargo.toml`/`package.json` from silently turning "no components"
/// into "one component spanning everything" and breaking the stated
/// backward-compatibility guarantee (`component: null` unchanged for a
/// repo with no *nested* project roots).
fn detect_components(repo_root: &Path, files: &[Node]) -> Result<Vec<Component>> {
    let dirs = collect_dirs(files);
    let mut candidates: Vec<(String, String)> = Vec::new(); // (path, kind)

    for (dir, info) in &dirs {
        if dir.is_empty() {
            continue;
        }
        let mut kind = None;
        for (marker, marker_kind) in EXACT_MARKERS {
            if info.basenames.contains(marker) {
                if is_aggregator(repo_root, dir, marker)? {
                    continue;
                }
                kind = Some(*marker_kind);
                break;
            }
        }
        if kind.is_none()
            && info
                .basenames
                .iter()
                .any(|b| b.ends_with(".csproj") || b.ends_with(".fsproj"))
        {
            kind = Some("dotnet");
        }
        if kind.is_none() && info.has_hcl {
            kind = Some("terraform");
        }
        if let Some(kind) = kind {
            candidates.push(((*dir).to_string(), kind.to_string()));
        }
    }

    Ok(name_candidates(candidates))
}

/// A `Cargo.toml`/`package.json` that only aggregates nested,
/// independently-versioned projects (a workspace root, an npm
/// `"workspaces"` root) is not itself a component — read the marker
/// file's own content (always small; these are manifest files, never
/// subject to `MAX_FILE_BYTES` concerns) and check for the aggregator
/// shape. A read failure (permissions, a race between `walk` and this
/// pass) is treated as "not an aggregator" — best-effort, since a
/// misclassified aggregator degrades to a slightly-too-eager component
/// rather than an invariant violation, not worth hard-failing the whole
/// index over.
fn is_aggregator(repo_root: &Path, dir: &str, marker: &str) -> Result<bool> {
    let path = repo_root.join(dir).join(marker);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    Ok(match marker {
        "Cargo.toml" => content.contains("[workspace]") && !content.contains("[package]"),
        "package.json" => content.contains("\"workspaces\""),
        _ => false,
    })
}

/// Assigns each candidate a name from its own directory's last path
/// segment, sanitized to `^[A-Za-z0-9][A-Za-z0-9_.-]*$`
/// (non-conforming characters become `-`; a non-alphanumeric leading
/// character gets a `c-` prefix). On a collision (two candidates whose
/// trailing segments produce the same name — `services/orders` and
/// `lambdas/orders` both wanting `"orders"`), extends *every* colliding
/// candidate by one more trailing path segment and retries
/// (`services-orders`/`lambdas-orders`) — symmetric, so which candidate
/// keeps the short name never depends on processing order, only on each
/// one's own path. Directory paths are unique by construction
/// (`collect_dirs` is keyed by path), so this always terminates: at
/// worst, a candidate's name grows to its full path with `/` replaced
/// by `-`.
fn name_candidates(mut candidates: Vec<(String, String)>) -> Vec<Component> {
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    let segments: Vec<Vec<String>> = candidates
        .iter()
        .map(|(path, _)| path.split('/').map(sanitize_segment).collect())
        .collect();
    let mut depth: Vec<usize> = vec![1; candidates.len()];

    loop {
        let names: Vec<String> = (0..candidates.len())
            .map(|i| trailing_join(&segments[i], depth[i]))
            .collect();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for n in &names {
            *counts.entry(n.as_str()).or_insert(0) += 1;
        }
        let mut any_collision = false;
        for i in 0..candidates.len() {
            if counts[names[i].as_str()] > 1 && depth[i] < segments[i].len() {
                depth[i] += 1;
                any_collision = true;
            }
        }
        if !any_collision {
            return candidates
                .into_iter()
                .zip(names)
                .map(|((path, kind), name)| Component { name, path, kind })
                .collect();
        }
    }
}

fn trailing_join(segments: &[String], depth: usize) -> String {
    let start = segments.len().saturating_sub(depth);
    segments[start..].join("-")
}

fn sanitize_segment(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if out.is_empty() || !out.chars().next().unwrap().is_ascii_alphanumeric() {
        out = format!("c-{out}");
    }
    out
}

#[derive(Deserialize)]
struct ConfigDoc {
    detect: Option<bool>,
    #[serde(default)]
    roots: Vec<ConfigRoot>,
}

#[derive(Deserialize)]
struct ConfigRoot {
    name: String,
    path: String,
    #[serde(default)]
    kind: Option<String>,
}

fn load_config(repo_root: &Path) -> Result<Option<ConfigDoc>> {
    let bytes = match read_config_bytes(repo_root)? {
        Some(b) => b,
        None => return Ok(None),
    };
    let doc: ConfigDoc = serde_json::from_slice(&bytes).map_err(|e| {
        Error::with_source(
            ErrorKind::UserError,
            format!(
                "failed to parse `{}`",
                repo_root.join(CONFIG_RELPATH).display()
            ),
            e,
        )
    })?;
    Ok(Some(doc))
}

fn read_config_bytes(repo_root: &Path) -> Result<Option<Vec<u8>>> {
    let config_path = repo_root.join(CONFIG_RELPATH);
    match std::fs::read(&config_path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::with_source(
            ErrorKind::UserError,
            format!("failed to read `{}`", config_path.display()),
            e,
        )),
    }
}

fn config_digest_for(bytes: Option<&[u8]>) -> String {
    crate::graph::id::blake3_hex_prefix(bytes.unwrap_or(b"<no .carto/roots.json>"))
}

const NAME_CHARSET_HINT: &str = "must match ^[A-Za-z0-9][A-Za-z0-9_.-]*$";

/// Validates and applies `config.roots`, replacing any auto-detected
/// component at the same `path` and hard-erroring on anything a typo
/// could plausibly produce — a `.carto/roots.json` entry is
/// repo-author intent, not a best-effort guess the way marker detection
/// is, so `detect`'s own aggregator/collision leniency doesn't apply
/// here.
fn apply_declared_roots(
    repo_root: &Path,
    files: &[Node],
    config: &ConfigDoc,
    detected: &mut Vec<Component>,
) -> Result<()> {
    let config_path = repo_root.join(CONFIG_RELPATH);
    let mut seen_names: BTreeMap<&str, &str> = BTreeMap::new(); // name -> path
    let mut seen_paths: BTreeMap<&str, &str> = BTreeMap::new(); // path -> name

    for root in &config.roots {
        if root.name.is_empty() || !valid_name_charset(&root.name) {
            return Err(Error::user(format!(
                "`{}`: invalid component name `{}` ({NAME_CHARSET_HINT})",
                config_path.display(),
                root.name
            )));
        }
        let path = root.path.trim_matches('/');
        if path.is_empty() || path.contains("..") || root.path.starts_with('/') {
            return Err(Error::user(format!(
                "`{}`: invalid component path `{}` (must be repo-relative, non-empty, no `..`)",
                config_path.display(),
                root.path
            )));
        }
        if !any_file_under(files, path) {
            return Err(Error::user(format!(
                "`{}`: component `{}` path `{path}` contains no walked file",
                config_path.display(),
                root.name
            )));
        }
        if let Some(&existing) = seen_names.get(root.name.as_str()) {
            if existing != path {
                return Err(Error::user(format!(
                    "`{}`: component name `{}` declared twice for different paths (`{existing}` and `{path}`)",
                    config_path.display(),
                    root.name
                )));
            }
        }
        if let Some(&existing) = seen_paths.get(path) {
            if existing != root.name {
                return Err(Error::user(format!(
                    "`{}`: path `{path}` declared twice under different names (`{existing}` and `{}`)",
                    config_path.display(),
                    root.name
                )));
            }
        }
        seen_names.insert(root.name.as_str(), path);
        seen_paths.insert(path, root.name.as_str());

        detected.retain(|c| c.path != path);
        detected.push(Component {
            name: root.name.clone(),
            path: path.to_string(),
            kind: root.kind.clone().unwrap_or_else(|| "custom".to_string()),
        });
    }
    Ok(())
}

fn valid_name_charset(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

fn any_file_under(files: &[Node], path: &str) -> bool {
    files.iter().any(|n| {
        n.data
            .as_file()
            .is_some_and(|f| f.path == path || f.path.starts_with(&format!("{path}/")))
    })
}

#[cfg(test)]
mod tests;
