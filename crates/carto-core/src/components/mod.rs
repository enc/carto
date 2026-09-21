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
//! repo-local `.carto/roots.json` (`detect`/`roots`/`exclude`),
//! following the exact built-ins-plus-repo-file shape
//! `crate::contracts::ContractRules` already established (ADR-0027):
//! JSON (no new dependency), closed-vocabulary fields hard-error naming
//! the file and the bad value, `kind` stays open.
//!
//! Terraform's own marker — "≥1 HCL file in the directory" — is the
//! weakest signal here (unlike every other marker, it names no single
//! project), so it alone gets a rollup pass (ADR-0037) that collapses a
//! normal `infra/modules/*`/`infra/envs/*`-shaped tree down to one
//! `infra` component instead of fragmenting into one candidate per
//! subdirectory — see [`detect_components`]'s own comment for the
//! rule.

mod deps;

use crate::error::{Error, ErrorKind, Result};
use crate::graph::Node;
use crate::lang::Lang;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Repo-relative path of the optional root-declaration override file.
const CONFIG_RELPATH: &str = ".carto/roots.json";

/// ADR-0039: component kinds this crate knows how to read a manifest
/// identity for (`Component::depends_on`, `deps::resolve`) —
/// `"terraform"`/`"custom"` have none, so a crossing *from* one of
/// those must never be flagged `"undeclared-dependency"` by
/// `lang::resolve` (there's no declared-dependency concept to have
/// violated; that would misrepresent "we never looked" as "confirmed
/// undeclared", exactly the absence-vs-zero confusion every
/// `SCHEMA_VERSION` bump in this family exists to avoid). `pub(crate)`
/// since both `lang::resolve` (evidence/resolution-preference) and
/// `query::map` (the `## cross-component edges` `[undeclared]` marker)
/// need the identical list.
pub(crate) const DEPENDENCY_AWARE_KINDS: &[&str] = &["go", "node", "rust", "dotnet", "php"];

/// One discovered or declared project root inside the walked tree.
/// `Serialize`/`Deserialize`: persisted verbatim as `GraphDocument::
/// components` (sorted by `path`, INV-7) — this is the on-disk shape,
/// not an internal detail either front end reshapes freely, the same
/// compatibility-surface status every other `graph.json`-visible type
/// already has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// ADR-0039: other components in this tree whose manifest this
    /// component's own manifest declares a dependency on (sorted
    /// names, deduplicated, self-references excluded) — see
    /// `crate::components::deps`'s own module doc for exactly what's
    /// parsed per `kind`. Empty for a `kind` this crate has no
    /// manifest-identity concept for (`"terraform"`, `"custom"`) — a
    /// real scope limit, not a claim of "confirmed no dependencies".
    /// `#[serde(default)]`: a v7-or-earlier `graph.json` never looked
    /// for this at all, the same absence-vs-zero distinction every
    /// prior `SCHEMA_VERSION` bump in this family protects — though
    /// `graph::load`'s exact-version check means no live query ever
    /// actually observes that default; it exists for direct
    /// `GraphDocument` construction in tests.
    #[serde(default)]
    pub depends_on: Vec<String>,
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
    /// strict ancestor of the other or disjoint — directory paths are
    /// unique by construction, and `apply_declared_roots` explicitly
    /// replaces any auto-detected entry at the same path rather than
    /// duplicating it), so "innermost" and "first match" are the same
    /// thing here.
    pub fn component_of_path(&self, file_path: &str) -> Option<&str> {
        self.components
            .iter()
            .find(|c| {
                file_path
                    .strip_prefix(c.path.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
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
        // Read the config file's bytes exactly once — both the parsed
        // `ConfigDoc` and `config_digest` derive from this same read,
        // rather than each independently re-reading the file (a real,
        // if narrow, TOCTOU: the file could change between two reads).
        let bytes = read_config_bytes(repo_root)?;
        let config = bytes
            .as_deref()
            .map(|b| parse_config(repo_root, b))
            .transpose()?;
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
            // ADR-0037: `exclude` runs before `roots` — a declared root
            // can still re-add a path the same config excludes, since
            // `apply_declared_roots` only ever adds/replaces, never
            // consults `exclude` itself.
            apply_excludes(repo_root, config, &mut detected)?;
            apply_declared_roots(repo_root, files, config, &mut detected)?;
        }

        detected.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.path.cmp(&b.path)));

        // ADR-0039: resolves each component's own manifest-declared
        // dependencies against every *other* component now that the
        // full set (auto-detected and declared) is final.
        deps::resolve(repo_root, &mut detected);

        Ok(ComponentSet {
            components: detected,
            config_digest: config_digest_for(bytes.as_deref()),
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
    // ADR-0037: every directory whose only marker is "has ≥1 HCL file"
    // is collected separately rather than pushed straight into
    // `candidates` the way every other kind is — terraform's rollup
    // pass below needs the *whole* set before deciding which
    // directories survive as their own component.
    let mut tf_dirs: BTreeSet<String> = BTreeSet::new();

    for (dir, info) in &dirs {
        if dir.is_empty() {
            continue;
        }
        let mut kind = None;
        for (marker, marker_kind) in EXACT_MARKERS {
            if info.basenames.contains(marker) {
                if is_aggregator(repo_root, dir, marker, &info.basenames)? {
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
        if let Some(kind) = kind {
            candidates.push(((*dir).to_string(), kind.to_string()));
        } else if info.has_hcl {
            tf_dirs.insert((*dir).to_string());
        }
    }

    // ADR-0037: a plain "any directory with ≥1 `.tf` file is its own
    // component" rule over-fragments a normal infra tree into one
    // candidate per module/environment directory (`infra/modules/vpc`,
    // `infra/envs/prod`, …) — generic names that collide with real
    // service names and defeat `--component`'s whole purpose for a
    // contracts/orphans query. Two passes fix that, applied only to the
    // terraform set (every other marker already names exactly one
    // project by construction, so needs neither):
    let strong_dirs: BTreeSet<&str> = candidates.iter().map(|(p, _)| p.as_str()).collect();
    // A terraform-only directory at or under an exact-marker
    // component's own directory belongs to that component, not to a
    // separate terraform one (`services/orders/infra` is part of
    // `orders`).
    tf_dirs.retain(|d| !is_at_or_under_any(d, &strong_dirs));
    // Repeatedly collapse the remainder to the lowest common ancestor
    // of any two surviving directories, provided that ancestor is
    // neither the repo root nor at-or-under a strong component's own
    // directory — until no further merge applies. A merge whose result
    // already equals one of its two inputs (`infra` absorbing
    // `infra/modules/vpc`) collapses a genuinely nested `.tf` tree to
    // its own topmost directory; a merge between two directories with
    // no shared `.tf`-bearing ancestor (`infra/envs/prod` and
    // `infra/modules/vpc`, once each has already collapsed to itself)
    // rolls scattered environment/module directories up to the one
    // directory that actually represents the infra project as a whole.
    // Two genuinely unrelated terraform trees (no shared ancestor short
    // of the repo root) are correctly left unmerged.
    let tf_dirs = merge_terraform_dirs(tf_dirs, &strong_dirs);
    for dir in tf_dirs {
        candidates.push((dir, "terraform".to_string()));
    }

    Ok(name_candidates(candidates))
}

/// `dir == ancestor`, or `dir` strictly nested under it at a segment
/// boundary — same rule as [`ComponentSet::component_of_path`]/
/// `any_file_under`, applied to two plain directory paths.
fn is_at_or_under(dir: &str, ancestor: &str) -> bool {
    dir == ancestor
        || dir
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn is_at_or_under_any(dir: &str, ancestors: &BTreeSet<&str>) -> bool {
    ancestors.iter().any(|a| is_at_or_under(dir, a))
}

/// The deepest directory that is a prefix of both `a` and `b` at a
/// segment boundary — `""` (the repo root) when they share no common
/// leading segment at all.
fn lowest_common_ancestor(a: &str, b: &str) -> String {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect::<Vec<_>>()
        .join("/")
}

/// See [`detect_components`]'s own comment for what this collapses and
/// why. `O(n^2)` per merge, `n` = directories containing ≥1 HCL file —
/// bounded by how many terraform-shaped subtrees a repo actually has,
/// not by repo size, so this stays cheap in practice. Deterministic:
/// `tf_dirs` is a `BTreeSet` throughout, so the same input always finds
/// the same first mergeable pair in the same order (INV-7).
fn merge_terraform_dirs(
    mut tf_dirs: BTreeSet<String>,
    strong_dirs: &BTreeSet<&str>,
) -> BTreeSet<String> {
    loop {
        let items: Vec<&String> = tf_dirs.iter().collect();
        let mut found: Option<(String, String, String)> = None;
        'search: for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let lca = lowest_common_ancestor(items[i], items[j]);
                if lca.is_empty() || is_at_or_under_any(&lca, strong_dirs) {
                    continue;
                }
                found = Some((items[i].clone(), items[j].clone(), lca));
                break 'search;
            }
        }
        match found {
            Some((a, b, lca)) => {
                tf_dirs.remove(&a);
                tf_dirs.remove(&b);
                tf_dirs.insert(lca);
            }
            None => return tf_dirs,
        }
    }
}

/// Sibling files, in the same directory as a `package.json`, whose
/// mere presence (ADR-0037) marks that `package.json` an aggregator
/// regardless of its own content — a pnpm/lerna/turborepo/nx/rush
/// workspace root declares its member packages in one of *these*
/// files, not in `package.json`'s own `"workspaces"` key, so content-
/// sniffing `package.json` alone would miss it. No parsing needed:
/// presence in the directory's own basenames is the whole signal.
const AGGREGATOR_SIBLING_MARKERS: &[&str] = &[
    "pnpm-workspace.yaml",
    "lerna.json",
    "turbo.json",
    "nx.json",
    "rush.json",
];

/// A `Cargo.toml`/`package.json` that only aggregates nested,
/// independently-versioned projects (a workspace root, an npm/pnpm/
/// lerna/turborepo/nx/rush workspace root) is not itself a component —
/// read the marker file's own content (always small; these are
/// manifest files, never subject to `MAX_FILE_BYTES` concerns) and
/// check for the aggregator shape, or (for `package.json`) check its
/// directory for one of [`AGGREGATOR_SIBLING_MARKERS`] first. A read
/// failure (permissions, a race between `walk` and this pass) is
/// treated as "not an aggregator" — best-effort, since a misclassified
/// aggregator degrades to a slightly-too-eager component rather than
/// an invariant violation, not worth hard-failing the whole index over.
fn is_aggregator(repo_root: &Path, dir: &str, marker: &str, basenames: &[&str]) -> Result<bool> {
    if marker == "package.json"
        && basenames
            .iter()
            .any(|b| AGGREGATOR_SIBLING_MARKERS.contains(b))
    {
        return Ok(true);
    }
    let path = repo_root.join(dir).join(marker);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    Ok(match marker {
        // Line-anchored, not a substring scan: a line that is *exactly*
        // `[workspace]`/`[package]` after trimming (comments and
        // indentation aside) — a substring match would also fire on an
        // unrelated `# see [workspace] docs` comment or a nested table
        // like `[workspace.dependencies]`, which is not itself a
        // `[workspace]` header. Deliberately not a `toml` dependency
        // (spec §13 + `deny.toml`'s `multiple-versions = "deny"`) — this
        // line scan is sufficient for the one shape being distinguished.
        "Cargo.toml" => {
            let mut has_workspace = false;
            let mut has_package = false;
            for line in content.lines() {
                match line.trim() {
                    l if l.starts_with('#') => {}
                    "[workspace]" => has_workspace = true,
                    "[package]" => has_package = true,
                    _ => {}
                }
            }
            has_workspace && !has_package
        }
        // Real JSON parsing rather than a `"workspaces"` substring scan
        // — `serde_json` is already a `carto-core` dependency. A
        // top-level `"workspaces"` key (npm/yarn's own convention) is
        // the aggregator signal; a parse failure falls through to
        // "not an aggregator" via `unwrap_or(false)`, same best-effort
        // treatment as an unreadable file.
        "package.json" => serde_json::from_str::<serde_json::Value>(&content)
            .ok()
            .and_then(|v| v.get("workspaces").map(|_| true))
            .unwrap_or(false),
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
                .map(|((path, kind), name)| Component {
                    name,
                    path,
                    kind,
                    ..Default::default()
                })
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
    /// ADR-0037: repo-relative paths to drop from auto-detection
    /// (`services/orders/infra`, say — a directory that has its own
    /// `.tf` files but should stay part of the `orders` component
    /// rather than becoming its own). Applied before `roots`, so a
    /// declared root can still re-add an excluded path. Unlike a
    /// `roots` entry's `path`, an exclude matching no auto-detected
    /// component is not an error — it's a legitimate "belt and braces"
    /// entry that simply has nothing to do yet.
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Deserialize)]
struct ConfigRoot {
    name: String,
    path: String,
    #[serde(default)]
    kind: Option<String>,
}

/// Parses already-read `.carto/roots.json` bytes — `repo_root` is used
/// only to name the file in an error message, not to read it again
/// (`discover` reads the file's bytes exactly once; see its own doc
/// comment).
fn parse_config(repo_root: &Path, bytes: &[u8]) -> Result<ConfigDoc> {
    serde_json::from_slice(bytes).map_err(|e| {
        Error::with_source(
            ErrorKind::UserError,
            format!(
                "failed to parse `{}`",
                repo_root.join(CONFIG_RELPATH).display()
            ),
            e,
        )
    })
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

/// ADR-0037: drops any auto-detected component at or under one of
/// `config.exclude`'s repo-relative paths. Validated the same way a
/// `roots` entry's `path` is (non-empty, repo-relative, no `..`
/// segment) — a config value is repo-author intent, not a best-effort
/// guess — except an exclude matching nothing is not an error (see
/// `ConfigDoc::exclude`'s own doc comment).
fn apply_excludes(
    repo_root: &Path,
    config: &ConfigDoc,
    detected: &mut Vec<Component>,
) -> Result<()> {
    let config_path = repo_root.join(CONFIG_RELPATH);
    for raw in &config.exclude {
        let path = raw.trim_matches('/');
        let is_dotdot_segment = path.split('/').any(|seg| seg == "..");
        if path.is_empty() || is_dotdot_segment || raw.starts_with('/') {
            return Err(Error::user(format!(
                "`{}`: invalid exclude path `{raw}` (must be repo-relative, non-empty, no `..` segment)",
                config_path.display()
            )));
        }
        detected.retain(|c| !is_at_or_under(&c.path, path));
    }
    Ok(())
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
        // `..` is only a traversal attempt as a whole path *segment* —
        // `contains("..")` alone would also reject a legitimately named
        // directory like `services/my..lib`.
        let is_dotdot_segment = path.split('/').any(|seg| seg == "..");
        if path.is_empty() || is_dotdot_segment || root.path.starts_with('/') {
            return Err(Error::user(format!(
                "`{}`: invalid component path `{}` (must be repo-relative, non-empty, no `..` segment)",
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
        // A declared name must also not collide with a *different*
        // auto-detected component's name (one whose path this same
        // declaration isn't about to replace) — otherwise two unrelated
        // directories would silently share one name downstream
        // (map.rs's per-name BTreeMap merges their counts;
        // resolve.rs's component-scoped tiers cross-match calls between
        // them).
        if let Some(other) = detected
            .iter()
            .find(|c| c.name == root.name && c.path != path)
        {
            return Err(Error::user(format!(
                "`{}`: component name `{}` collides with an auto-detected component at `{}` — rename one or add it to `roots` explicitly",
                config_path.display(),
                root.name,
                other.path
            )));
        }
        seen_names.insert(root.name.as_str(), path);
        seen_paths.insert(path, root.name.as_str());

        detected.retain(|c| c.path != path);
        detected.push(Component {
            name: root.name.clone(),
            path: path.to_string(),
            kind: root.kind.clone().unwrap_or_else(|| "custom".to_string()),
            ..Default::default()
        });
    }
    Ok(())
}

fn valid_name_charset(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// Same segment-boundary prefix rule as
/// [`ComponentSet::component_of_path`], applied to a plain `path`
/// string rather than a `NodeId` — `strip_prefix` + boundary check
/// rather than a per-file `format!("{path}/")` allocation, which this
/// was previously doing once per walked file per declared root.
fn any_file_under(files: &[Node], path: &str) -> bool {
    files.iter().any(|n| {
        n.data.as_file().is_some_and(|f| {
            f.path
                .strip_prefix(path)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
    })
}

#[cfg(test)]
mod tests;
