//! `carto map --budget <lines>` (spec §7.1: "layered overview: top
//! modules by fan-in/out, entry points, infra summary, join stats; hard-
//! capped at budget"). Unlike `find`/`deps`, which return structured
//! rows for a caller to interpret, `map`'s core output is already
//! rendered text — spec frames the budget explicitly in terms of
//! *lines*, so the truncation decision has to be made against the
//! rendered form, not an abstract row count. `counts` stays a separate
//! structured field (numbers, not string-parsed) since giving `--json`
//! consumers exact counts is nearly free and doesn't compete for budget.
//!
//! Sections render in spec §7.1's order, each only if it fits the
//! remaining budget; the first section that doesn't fit stops the
//! listing entirely rather than skipping ahead to a smaller later
//! section — sections are meant to be read as coherent blocks.

use super::{QueryGraph, Truncation};
use crate::consts;
use crate::graph::{Confidence, EdgeKind, NodeData, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How many rows the fan-in/out ranking keeps per category (files,
/// external packages) — the overview is meant to be a summary, not a
/// full listing; `deps`/`where` are for exhaustive traversal.
const RANKED_ROWS: usize = 10;

/// One of `map`'s rendered sections, in spec §7.1's fixed order — §2.1's
/// `--section` filter (ADR to follow) lets a caller ask for a subset
/// instead of always paying for the full overview, the clearest, cheapest
/// token-efficiency win the S-1 benchmark's replay arm found: a narrowly-
/// scoped question (e.g. "what does this repo depend on externally")
/// only ever needed one section, but every call rendered all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapSection {
    Counts,
    Modules,
    EntryPoints,
    Infra,
    /// ADR-0034/0035: the repo's top-level component structure — one
    /// row per component plus a cross-component edge summary. Placed
    /// last in spec §7.1's original section order (which predates this
    /// section entirely) rather than reordering the other four.
    Components,
}

impl MapSection {
    /// The spelling accepted by `--section` and used when composing a
    /// `Truncation::next_call` — mirrors [`EdgeKind::as_str`]'s role for
    /// `deps --kinds`.
    pub fn as_str(self) -> &'static str {
        match self {
            MapSection::Counts => "counts",
            MapSection::Modules => "modules",
            MapSection::EntryPoints => "entry-points",
            MapSection::Infra => "infra",
            MapSection::Components => "components",
        }
    }

    /// Inverse of [`as_str`](Self::as_str). `None` for anything else.
    pub fn parse(s: &str) -> Option<MapSection> {
        match s {
            "counts" => Some(MapSection::Counts),
            "modules" => Some(MapSection::Modules),
            "entry-points" => Some(MapSection::EntryPoints),
            "infra" => Some(MapSection::Infra),
            "components" => Some(MapSection::Components),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MapQuery {
    pub budget: u32,
    /// Restrict the overview to this repo-relative directory (spec
    /// §7.1's `--subpath`) — see [`QueryGraph::path_in_scope`] and this
    /// module's per-section application of it: each section's
    /// underlying computation (a file's real fan-in/out, whether some
    /// *other* file outside the subtree already imports a candidate
    /// entry point) stays whole-graph-accurate; only which rows get
    /// *listed* is restricted. `None` means no restriction — and every
    /// section reduces to today's exact unscoped behavior in that case,
    /// not an approximation of it.
    pub subpath: Option<String>,
    /// Restrict rendering to these sections (§2.1) — `None` (the
    /// default) keeps today's full-overview behavior, additive rather
    /// than a breaking change to the existing contract. `counts` (the
    /// structured field) is always computed and returned regardless of
    /// this filter; only which `lines` render is affected.
    pub sections: Option<BTreeSet<MapSection>>,
    /// Restrict rendering to these components (ADR-0034/0035,
    /// `--component`, repeatable) — same "restricts what's listed,
    /// never what's computed" principle as `subpath`, applied alongside
    /// it (independent dimensions; both given means both apply). `None`
    /// or empty means no restriction.
    pub component: Option<BTreeSet<String>>,
}

impl MapQuery {
    pub fn new() -> Self {
        MapQuery {
            budget: consts::DEFAULT_MAP_BUDGET,
            subpath: None,
            sections: None,
            component: None,
        }
    }
}

impl Default for MapQuery {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapCounts {
    pub files: usize,
    pub symbols: usize,
    pub modules: usize,
    /// Keyed by `EdgeKind::as_str()` rather than `EdgeKind` itself —
    /// sidesteps relying on serde_json's enum-as-object-key behavior for
    /// a field that has no need to round-trip as anything but display
    /// data.
    pub edges_by_kind: BTreeMap<String, usize>,
    /// ADR-0034/0035: per in-scope component (subject to `--subpath`/
    /// `--component`), keyed by name. Empty for a repo with no
    /// components — a real, meaningful fact (this is a single-project
    /// repo, or `.carto/roots.json`/marker detection found nothing),
    /// not an unfilled placeholder.
    pub components: BTreeMap<String, ComponentCounts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentCounts {
    pub path: String,
    pub kind: String,
    pub files: usize,
    pub symbols: usize,
    /// ADR-0039: this component's own manifest-declared dependencies
    /// (sorted names) — empty both for a component with genuinely no
    /// declared dependency and for a `kind` carto has no manifest-
    /// identity concept for (`"terraform"`/`"custom"`); this field
    /// alone can't distinguish those two, the same limit
    /// `Component::depends_on`'s own doc comment names.
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapResult {
    pub counts: MapCounts,
    /// The budget-capped, pre-rendered overview — one string per line.
    pub lines: Vec<String>,
    pub truncation: Truncation,
}

pub fn run(qg: &QueryGraph, query: &MapQuery) -> MapResult {
    let subpath = query.subpath.as_deref();
    let component = query.component.as_ref();
    let counts = compute_counts(qg, subpath, component);

    // `counts` (the structured field) stays exact and whole regardless
    // of `--section` (§2.2: the machine payload is never trimmed, only
    // `--section` lets a caller ask for less of the rendered text) —
    // this filter only decides which of these five blocks contribute to
    // `lines`.
    let all_sections: Vec<(MapSection, Vec<String>)> = vec![
        (MapSection::Counts, counts_lines(&counts)),
        (
            MapSection::Modules,
            top_modules_lines(qg, subpath, component),
        ),
        (
            MapSection::EntryPoints,
            entry_points_lines(qg, subpath, component),
        ),
        (MapSection::Infra, infra_lines(qg, subpath, component)),
        (MapSection::Components, components_lines(&counts, qg)),
    ];
    let sections: Vec<Vec<String>> = all_sections
        .into_iter()
        .filter(|(tag, _)| query.sections.as_ref().is_none_or(|s| s.contains(tag)))
        .map(|(_, lines)| lines)
        .collect();

    let budget = query.budget as usize;
    let mut lines: Vec<String> = Vec::new();
    let mut dropped_any = false;
    for section in sections {
        if lines.len() + section.len() <= budget {
            lines.extend(section);
        } else {
            dropped_any = true;
            break;
        }
    }

    let truncation = if dropped_any {
        let next_budget = if query.budget == 0 {
            consts::DEFAULT_MAP_BUDGET
        } else {
            query.budget.saturating_mul(2)
        };
        // Carries --section forward too, if set — the same "the exact
        // follow-up call reproduces the same scoped view" precedent
        // `deps`'s --subpath truncation already set.
        let section_flags = query
            .sections
            .as_ref()
            .map(|s| {
                s.iter()
                    .map(|sec| format!(" --section {}", sec.as_str()))
                    .collect::<String>()
            })
            .unwrap_or_default();
        let component_flags = QueryGraph::component_flags(query.component.as_ref());
        Truncation::more(format!(
            "carto map --budget {next_budget}{section_flags}{component_flags}"
        ))
    } else {
        Truncation::none()
    };

    MapResult {
        counts,
        lines,
        truncation,
    }
}

fn compute_counts(
    qg: &QueryGraph,
    subpath: Option<&str>,
    component: Option<&BTreeSet<String>>,
) -> MapCounts {
    let mut files = 0;
    let mut symbols = 0;
    let mut components = seed_component_counts(qg, subpath, component);
    for node in qg.nodes() {
        // Per-component tallying is independent of the overall
        // files/symbols counters just below -- a component's own
        // counts are scoped by whether *the component itself* passed
        // --subpath/--component (`seed_component_counts` already
        // decided that; `components.get_mut` is the only gate here),
        // not by re-applying those filters per node. Folded into this
        // same loop rather than a second pass over `qg.nodes()`, since
        // both need the identical `component_of` lookup per node.
        if let Some(name) = qg.component_of(&node.id) {
            if let Some(entry) = components.get_mut(name) {
                match &node.data {
                    NodeData::File(_) => entry.files += 1,
                    NodeData::Symbol(_) => entry.symbols += 1,
                    NodeData::Module(_) | NodeData::Contract(_) => {}
                }
            }
        }

        if !qg.path_in_scope(&node.id, subpath) || !qg.component_in_scope(&node.id, component) {
            continue;
        }
        match &node.data {
            NodeData::File(_) => files += 1,
            NodeData::Symbol(_) => symbols += 1,
            // `map`'s counts don't cover Contract nodes this slice
            // (ADR-0026) — `orphans` is the dedicated surface for them.
            NodeData::Module(_) | NodeData::Contract(_) => {}
        }
    }

    // Modules aren't excluded by path_in_scope directly (they have no
    // directory), so they're counted here via the edges that actually
    // touch them instead — a module is counted iff at least one of its
    // edges has an in-scope endpoint. Every Module node in the graph
    // always has >=1 edge by construction (resolve.rs only ever creates
    // one alongside the edge that references it), so this reduces to
    // "count every Module node" when `subpath` is `None` — identical to
    // this function's pre-`--subpath` behavior, not an approximation.
    let mut touched_modules: BTreeSet<NodeId> = BTreeSet::new();
    let mut edges_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for edge in qg.edges() {
        let from_in =
            qg.path_in_scope(&edge.from, subpath) && qg.component_in_scope(&edge.from, component);
        let to_in =
            qg.path_in_scope(&edge.to, subpath) && qg.component_in_scope(&edge.to, component);
        if !from_in && !to_in {
            continue;
        }
        *edges_by_kind
            .entry(edge.kind.as_str().to_string())
            .or_insert(0) += 1;
        if matches!(
            qg.node(&edge.to).map(|n| &n.data),
            Some(NodeData::Module(_))
        ) {
            touched_modules.insert(edge.to.clone());
        }
    }

    MapCounts {
        files,
        symbols,
        modules: touched_modules.len(),
        edges_by_kind,
        components,
    }
}

/// ADR-0034/0035: seeds one zeroed `ComponentCounts` entry per in-scope
/// component — a component is in scope iff its own `path` passes
/// `--subpath` and its `name` passes `--component`, the same two
/// filters every other section already applies to individual nodes,
/// applied here to the component entry itself since a `Component` isn't
/// a graph node `path_in_scope`/`component_in_scope` can look up
/// directly. The actual file/symbol tallying happens in
/// `compute_counts`'s own node loop, not here — folding the two into
/// one pass over `qg.nodes()` avoids re-deriving each node's
/// `component_of` twice.
fn seed_component_counts(
    qg: &QueryGraph,
    subpath: Option<&str>,
    component: Option<&BTreeSet<String>>,
) -> BTreeMap<String, ComponentCounts> {
    let mut out: BTreeMap<String, ComponentCounts> = BTreeMap::new();
    for c in qg.components() {
        if !subpath_matches(&c.path, subpath) {
            continue;
        }
        if let Some(filter) = component {
            // ADR-0038: descendant-aware, not a bare name-set membership
            // check — `--component infra` must still seed `infra`'s own
            // nested components (if any), not just an exact `"infra"`
            // row.
            if !filter.is_empty() && !qg.component_matches_filter(&c.name, filter) {
                continue;
            }
        }
        out.insert(
            c.name.clone(),
            ComponentCounts {
                path: c.path.clone(),
                kind: c.kind.clone(),
                files: 0,
                symbols: 0,
                depends_on: c.depends_on.clone(),
            },
        );
    }
    out
}

/// [`QueryGraph::path_in_scope`]'s exact segment-boundary rule, applied
/// to a plain path string rather than a `NodeId` — needed for a
/// `Component`'s own `path`, which isn't itself a graph node.
fn subpath_matches(path: &str, subpath: Option<&str>) -> bool {
    let prefix = match subpath.map(str::trim) {
        None | Some("") => return true,
        Some(p) => p,
    };
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn counts_lines(counts: &MapCounts) -> Vec<String> {
    let mut lines = vec![format!(
        "files={} symbols={} modules={}",
        counts.files, counts.symbols, counts.modules
    )];
    if !counts.edges_by_kind.is_empty() {
        let parts: Vec<String> = counts
            .edges_by_kind
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        lines.push(format!("edges: {}", parts.join(" ")));
    }
    lines
}

/// `File` nodes ranked by `imports` fan-in + fan-out, plus external
/// `Module` nodes ranked by fan-in ("which third-party packages this
/// repo leans on"). Spec §7.1: "top modules by fan-in/out".
fn top_modules_lines(
    qg: &QueryGraph,
    subpath: Option<&str>,
    component: Option<&BTreeSet<String>>,
) -> Vec<String> {
    let mut file_fan: BTreeMap<NodeId, (usize, usize)> = BTreeMap::new(); // (in, out)
    for node in qg.nodes() {
        if matches!(node.data, NodeData::File(_)) {
            file_fan.insert(node.id.clone(), (0, 0));
        }
    }
    let mut module_fan_in: BTreeMap<NodeId, usize> = BTreeMap::new();
    for node in qg.nodes() {
        if let NodeData::Module(m) = &node.data {
            if m.external {
                module_fan_in.insert(node.id.clone(), 0);
            }
        }
    }

    for edge in qg.edges() {
        if edge.kind != EdgeKind::Imports {
            continue;
        }
        // File fan-in/out stays whole-graph-accurate regardless of
        // `--subpath` — a file's real connectivity, not clipped at the
        // subtree boundary; only which *rows* get listed is restricted,
        // below.
        if let Some(entry) = file_fan.get_mut(&edge.from) {
            entry.1 += 1;
        }
        if let Some(entry) = file_fan.get_mut(&edge.to) {
            entry.0 += 1;
        }
        // External-package fan-in, deliberately different: only count
        // an edge whose *source* file is in scope — "what does this
        // subtree depend on externally" is the useful question here,
        // not a whole-repo number (ADR-0014, extended to `--component`
        // the same way for ADR-0034/0035).
        if qg.path_in_scope(&edge.from, subpath) && qg.component_in_scope(&edge.from, component) {
            if let Some(entry) = module_fan_in.get_mut(&edge.to) {
                *entry += 1;
            }
        }
    }

    let mut lines = vec!["## top modules (imports fan-in/out)".to_string()];

    let mut ranked_files: Vec<(String, usize, usize)> = file_fan
        .into_iter()
        .filter(|(id, (inn, out))| {
            (*inn > 0 || *out > 0)
                && qg.path_in_scope(id, subpath)
                && qg.component_in_scope(id, component)
        })
        .filter_map(|(id, (inn, out))| {
            let path = qg.node(&id).and_then(|n| n.data.as_file())?.path.clone();
            Some((path, inn, out))
        })
        .collect();
    ranked_files.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)).then_with(|| a.0.cmp(&b.0)));
    ranked_files.truncate(RANKED_ROWS);
    if ranked_files.is_empty() {
        lines.push("  (none)".to_string());
    }
    for (path, inn, out) in ranked_files {
        lines.push(format!("  {path}  in={inn} out={out}"));
    }

    let mut ranked_modules: Vec<(String, usize)> = module_fan_in
        .into_iter()
        .filter(|(_, fan_in)| *fan_in > 0)
        .filter_map(|(id, fan_in)| {
            let path = match &qg.node(&id)?.data {
                NodeData::Module(m) => m.path.clone(),
                _ => return None,
            };
            Some((path, fan_in))
        })
        .collect();
    ranked_modules.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked_modules.truncate(RANKED_ROWS);
    if !ranked_modules.is_empty() {
        lines.push("## external packages (imports fan-in)".to_string());
        for (path, fan_in) in ranked_modules {
            lines.push(format!("  {path}  in={fan_in}"));
        }
    }

    lines
}

/// Structural heuristic (spec §7.1: "entry points"), labeled as such:
/// `File` nodes with zero incoming `imports` edges, plus any symbol
/// named `main`. Neither is a real entry-point analysis (that needs
/// language-specific knowledge — e.g. a `Cargo.toml` `[[bin]]` table —
/// that carto doesn't have in v1); this is the same-spirit "honest
/// heuristic, clearly labeled" pattern spec §7.1 uses for
/// `unused_permissions`.
fn entry_points_lines(
    qg: &QueryGraph,
    subpath: Option<&str>,
    component: Option<&BTreeSet<String>>,
) -> Vec<String> {
    // Stays whole-graph: a file imported only from *outside* the
    // subtree must not look like a false entry point just because
    // that importer isn't listed.
    let mut has_incoming_import: BTreeSet<NodeId> = BTreeSet::new();
    for edge in qg.edges() {
        if edge.kind == EdgeKind::Imports {
            has_incoming_import.insert(edge.to.clone());
        }
    }

    let mut lines =
        vec!["## entry points (heuristic: no incoming imports, or named `main`)".to_string()];

    let mut candidates: Vec<String> = Vec::new();
    for node in qg.nodes() {
        if let NodeData::File(f) = &node.data {
            if !has_incoming_import.contains(&node.id)
                && qg.path_in_scope(&node.id, subpath)
                && qg.component_in_scope(&node.id, component)
            {
                candidates.push(f.path.clone());
            }
        }
    }
    candidates.sort();

    let mut main_fns: Vec<String> = Vec::new();
    for node in qg.nodes() {
        if let NodeData::Symbol(s) = &node.data {
            if s.name == "main"
                && qg.path_in_scope(&node.id, subpath)
                && qg.component_in_scope(&node.id, component)
            {
                main_fns.push(qg.location(s));
            }
        }
    }
    main_fns.sort();

    if candidates.is_empty() && main_fns.is_empty() {
        lines.push("  (none found)".to_string());
    } else {
        for path in candidates {
            lines.push(format!("  {path}"));
        }
        for loc in main_fns {
            lines.push(format!("  {loc} (fn main)"));
        }
    }

    lines
}

/// The `infra`/`join` sections. Without any Terraform symbol in scope
/// these stay the explicit placeholders they have always been —
/// an absent section reads as "no infra found"; a present one saying
/// "not implemented" is honest about why (spec §10: infra is M2, the
/// join is M3). With Terraform present (ADR-0041/0042) the `infra`
/// section summarizes what *source-level* Terraform support can say —
/// symbol counts, the module-call graph, remote modules — and states
/// plainly that this is not a resolved plan (no `IacResource`,
/// `depends_on` or IAM: those need plan/state-JSON ingestion, M2).
fn infra_lines(
    qg: &QueryGraph,
    subpath: Option<&str>,
    component: Option<&BTreeSet<String>>,
) -> Vec<String> {
    let in_scope = |id: &crate::graph::NodeId| {
        qg.path_in_scope(id, subpath) && qg.component_in_scope(id, component)
    };

    let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
    for node in qg.nodes() {
        if let NodeData::Symbol(s) = &node.data {
            if s.sym_kind.is_terraform() && in_scope(&node.id) {
                *by_kind.entry(s.sym_kind.as_str()).or_insert(0) += 1;
            }
        }
    }
    let join = [
        "## join".to_string(),
        "  none — requires M3 (code<->infra join)".to_string(),
    ];
    if by_kind.is_empty() {
        let mut lines = vec![
            "## infra".to_string(),
            "  none — requires M2 (infrastructure graph)".to_string(),
        ];
        lines.extend(join);
        return lines;
    }

    let total: usize = by_kind.values().sum();
    let kinds: Vec<String> = by_kind.iter().map(|(k, n)| format!("{n} {k}")).collect();
    let mut lines = vec![
        "## infra".to_string(),
        format!(
            "  source-level Terraform (parsed .tf files, not a resolved plan): {total} symbols — {}",
            kinds.join(", ")
        ),
    ];

    // Module-call graph: `imports` edges the Terraform pass tagged.
    let dir_of = |id: &crate::graph::NodeId| -> Option<String> {
        let path = &qg.node(id)?.data.as_file()?.path;
        Some(
            path.rsplit_once('/')
                .map(|(d, _)| d)
                .unwrap_or("")
                .to_string(),
        )
    };
    let mut calls: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut unit_deps: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut remote: BTreeMap<String, usize> = BTreeMap::new();
    for edge in qg.edges() {
        if edge.kind != EdgeKind::Imports || !in_scope(&edge.from) {
            continue;
        }
        match edge.evidence.first().map(String::as_str) {
            // A Terragrunt unit's `terraform.source` is a module call too.
            Some("tf-module-source" | "tg-terraform-source") => {
                if let (Some(from), Some(to)) = (dir_of(&edge.from), dir_of(&edge.to)) {
                    *calls.entry((from, to)).or_insert(0) += 1;
                }
            }
            Some("tg-dependency" | "tg-dependencies") => {
                if let (Some(from), Some(to)) = (dir_of(&edge.from), dir_of(&edge.to)) {
                    *unit_deps.entry((from, to)).or_insert(0) += 1;
                }
            }
            Some("tf-module-remote" | "tg-terraform-source:remote") => {
                if let Some(NodeData::Module(m)) = qg.node(&edge.to).map(|n| &n.data) {
                    *remote.entry(m.path.clone()).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }
    if !calls.is_empty() {
        let mut ranked: Vec<_> = calls.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(RANKED_ROWS);
        lines.push("  module calls (calling dir -> called dir, file-level imports):".to_string());
        for ((from, to), n) in ranked {
            let from = if from.is_empty() { "." } else { from.as_str() };
            let to = if to.is_empty() { "." } else { to.as_str() };
            lines.push(format!("    {from} -> {to}  ({n})"));
        }
    }
    if !unit_deps.is_empty() {
        let mut ranked: Vec<_> = unit_deps.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(RANKED_ROWS);
        lines.push("  terragrunt dependencies (unit dir -> dependency unit dir):".to_string());
        for ((from, to), n) in ranked {
            lines.push(format!("    {from} -> {to}  ({n})"));
        }
    }
    if !remote.is_empty() {
        let mut ranked: Vec<_> = remote.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(RANKED_ROWS);
        lines.push(
            "  remote modules (registry/git/http sources, credentials stripped):".to_string(),
        );
        for (path, n) in ranked {
            lines.push(format!("    {path}  in={n}"));
        }
    }
    lines.push(
        "  resolved infrastructure graph (IacResource, depends_on, IAM): none — requires M2 (plan/state JSON ingestion)"
            .to_string(),
    );
    lines.extend(join);
    lines
}

/// ADR-0034/0035: one row per in-scope component (from `counts.components`,
/// already filtered by `--subpath`/`--component`), plus a summary of
/// edges crossing between two *different* components — always printing
/// the header, matching every other section's "(none)"-when-empty
/// convention (`top_modules_lines`/`entry_points_lines` above) rather
/// than omitting the section, so a repo with no components reports that
/// honestly instead of looking like the question was never asked.
fn components_lines(counts: &MapCounts, qg: &QueryGraph) -> Vec<String> {
    let mut lines = vec!["## components".to_string()];
    if counts.components.is_empty() {
        lines.push("  (none)".to_string());
        return lines;
    }
    for (name, c) in &counts.components {
        let depends_on = if c.depends_on.is_empty() {
            "(none)".to_string()
        } else {
            c.depends_on.join(", ")
        };
        lines.push(format!(
            "  {name}  path={} kind={} files={} symbols={} depends_on={depends_on}",
            c.path, c.kind, c.files, c.symbols
        ));
    }

    // Edges whose endpoints sit in two *different* components, tallied
    // by (from-component, to-component, kind, confidence) — restricted
    // to pairs touching at least one in-scope component
    // (`counts.components`' own keys), the same "restrict what's
    // listed" principle every other section applies, not a second
    // independent filter. Confidence is part of the tally key, not
    // folded away, so a `certain` crossing (PHP `use`/C# `using`,
    // ADR-0034's own Context names these "the most damaging kind") is
    // never lumped in with an `inferred` `calls` crossing under one
    // count — the whole point of this section is telling those apart.
    type CrossTally = BTreeMap<(String, String), BTreeMap<(&'static str, Confidence), usize>>;
    let mut cross: CrossTally = BTreeMap::new();
    for edge in qg.edges() {
        let (Some(from_c), Some(to_c)) = (qg.component_of(&edge.from), qg.component_of(&edge.to))
        else {
            continue;
        };
        if from_c == to_c {
            continue;
        }
        if !counts.components.contains_key(from_c) && !counts.components.contains_key(to_c) {
            continue;
        }
        *cross
            .entry((from_c.to_string(), to_c.to_string()))
            .or_default()
            .entry((edge.kind.as_str(), edge.confidence))
            .or_insert(0) += 1;
    }
    if !cross.is_empty() {
        // ADR-0039: `[undeclared]` when `from_c`'s own component is a
        // dependency-aware kind (see `crate::components::
        // DEPENDENCY_AWARE_KINDS`'s doc comment) and `to_c` isn't in
        // its declared `depends_on` — a component-level summary of the
        // same per-edge `"undeclared-dependency"` evidence entry
        // `lang::resolve` adds, not a second independent computation:
        // any edge in this pair carrying that evidence implies the
        // pair itself qualifies, since the check is identical
        // (component-to-component, not edge-specific).
        let component_by_name: BTreeMap<&str, &crate::components::Component> = qg
            .components()
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        lines.push("## cross-component edges".to_string());
        for ((from_c, to_c), kinds) in &cross {
            let parts: Vec<String> = kinds
                .iter()
                .map(|((k, conf), v)| format!("{v} {k}({})", conf.as_str()))
                .collect();
            let undeclared = component_by_name.get(from_c.as_str()).is_some_and(|c| {
                crate::components::DEPENDENCY_AWARE_KINDS.contains(&c.kind.as_str())
                    && !c.depends_on.iter().any(|d| d == to_c)
            });
            let marker = if undeclared { "  [undeclared]" } else { "" };
            lines.push(format!(
                "  {from_c} -> {to_c}: {}{marker}",
                parts.join(", ")
            ));
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Confidence, Edge, FileNode, GraphDocument, Node, SymKind, SymbolNode};
    use crate::lang::Lang;
    use crate::taint::Provenance;

    fn file(path: &str) -> Node {
        Node::file(
            crate::graph::file_id(path),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: path.to_string(),
                lang: Lang::Rust,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
                component: None,
            },
        )
    }

    fn symbol(relpath: &str, name: &str, file_id: &NodeId, line: u32) -> Node {
        Node::symbol(
            crate::graph::sym_id(relpath, "function", name, line),
            Provenance::Syntactic,
            "test@1",
            SymbolNode {
                name: name.to_string(),
                sym_kind: SymKind::Function,
                file: file_id.clone(),
                start_line: line,
                end_line: line + 1,
                signature: None,
                unresolved_calls: vec![],
                uncaptured_inbound_calls: 0,
                uncaptured_outbound_calls: 0,
                unresolved_inbound_calls: vec![],
                unresolved_inbound_call_count: 0,
            },
        )
    }

    fn doc(nodes: Vec<Node>, edges: Vec<Edge>) -> GraphDocument {
        GraphDocument {
            carto_version: "0.1.0".to_string(),
            schema_version: crate::consts::SCHEMA_VERSION,
            components: vec![],
            nodes,
            edges,
        }
    }

    fn module_node(path: &str) -> Node {
        Node::module(
            crate::graph::module_id(path, true),
            Provenance::Syntactic,
            "test@1",
            crate::graph::ModuleNode {
                path: path.to_string(),
                external: true,
            },
        )
    }

    /// `mg_site/handlers.rs` imports `mg_site/orders.rs` (in-subtree)
    /// and external package `serde`; `vendor/legacy.rs` (out of
    /// subtree) imports both `mg_site/orders.rs` and external package
    /// `lodash` — for `--subpath mg_site` tests: an out-of-scope
    /// importer keeping an in-scope file's fan-in real, and an
    /// out-of-scope-only external dependency that must not show up in
    /// a scoped external-package ranking.
    fn subtree_doc() -> GraphDocument {
        let handlers = file("mg_site/handlers.rs");
        let orders = file("mg_site/orders.rs");
        let legacy = file("vendor/legacy.rs");
        let serde = module_node("serde");
        let lodash = module_node("lodash");

        let handlers_imports_orders = Edge::new(
            EdgeKind::Imports,
            handlers.id.clone(),
            orders.id.clone(),
            Confidence::Certain,
            "mod-declaration".to_string(),
        );
        let handlers_imports_serde = Edge::new(
            EdgeKind::Imports,
            handlers.id.clone(),
            serde.id.clone(),
            Confidence::Certain,
            "external-package".to_string(),
        );
        let legacy_imports_orders = Edge::new(
            EdgeKind::Imports,
            legacy.id.clone(),
            orders.id.clone(),
            Confidence::Certain,
            "mod-declaration".to_string(),
        );
        let legacy_imports_lodash = Edge::new(
            EdgeKind::Imports,
            legacy.id.clone(),
            lodash.id.clone(),
            Confidence::Certain,
            "external-package".to_string(),
        );

        doc(
            vec![handlers, orders, legacy, serde, lodash],
            vec![
                handlers_imports_orders,
                handlers_imports_serde,
                legacy_imports_orders,
                legacy_imports_lodash,
            ],
        )
    }

    /// `lib.rs` imports `orders.rs`; `orders.rs` has an unrelated symbol.
    /// `lib.rs` has no incoming imports (entry point); `orders.rs` does.
    fn small_repo_doc() -> GraphDocument {
        let lib = file("src/lib.rs");
        let orders = file("src/orders.rs");
        let sym = symbol("src/orders.rs", "parse_order", &orders.id, 1);
        let import_edge = Edge::new(
            EdgeKind::Imports,
            lib.id.clone(),
            orders.id.clone(),
            Confidence::Certain,
            "mod-declaration".to_string(),
        );
        let contains_edge = Edge::new(
            EdgeKind::Contains,
            orders.id.clone(),
            sym.id.clone(),
            Confidence::Certain,
            "extractor:rust".to_string(),
        );
        doc(vec![lib, orders, sym], vec![import_edge, contains_edge])
    }

    #[test]
    fn counts_reflect_node_and_edge_kinds() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let result = run(&qg, &MapQuery::new());
        assert_eq!(result.counts.files, 2);
        assert_eq!(result.counts.symbols, 1);
        assert_eq!(result.counts.modules, 0);
        assert_eq!(result.counts.edges_by_kind.get("imports"), Some(&1));
        assert_eq!(result.counts.edges_by_kind.get("contains"), Some(&1));
    }

    #[test]
    fn subpath_restricts_file_counts_to_the_subtree() {
        let qg = QueryGraph::from_document(subtree_doc());
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: Some("mg_site".to_string()),
                sections: None,
                component: None,
            },
        );
        // handlers.rs + orders.rs, not vendor/legacy.rs.
        assert_eq!(result.counts.files, 2);
    }

    #[test]
    fn subpath_keeps_a_files_real_fan_in_in_the_ranking_even_from_an_out_of_scope_importer() {
        let qg = QueryGraph::from_document(subtree_doc());
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: Some("mg_site".to_string()),
                sections: None,
                component: None,
            },
        );
        let joined = result.lines.join("\n");
        // orders.rs is imported by both handlers.rs (in scope) and
        // vendor/legacy.rs (out of scope) — its listed fan-in must
        // still be the real total (2), not clipped to only in-scope
        // importers.
        assert!(joined.contains("mg_site/orders.rs  in=2 out=0"), "{joined}");
        // vendor/legacy.rs itself must not appear as a row.
        assert!(!joined.contains("vendor/legacy.rs"));
    }

    #[test]
    fn subpath_scopes_external_package_fan_in_to_in_scope_importers_only() {
        let qg = QueryGraph::from_document(subtree_doc());
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: Some("mg_site".to_string()),
                sections: None,
                component: None,
            },
        );
        let joined = result.lines.join("\n");
        // serde is imported by handlers.rs (in scope) -> shown.
        assert!(joined.contains("serde"), "{joined}");
        // lodash is imported only by vendor/legacy.rs (out of scope)
        // -> not shown, unlike the file fan-in case above, which
        // deliberately stays whole-graph-accurate (see ADR-0014).
        assert!(!joined.contains("lodash"), "{joined}");
    }

    #[test]
    fn subpath_does_not_falsely_list_a_file_as_an_entry_point_when_only_an_out_of_scope_file_imports_it()
     {
        let qg = QueryGraph::from_document(subtree_doc());
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: Some("mg_site".to_string()),
                sections: None,
                component: None,
            },
        );
        let joined = result.lines.join("\n");
        let entry_section = joined.split("## entry points").nth(1).unwrap();
        let entry_section = entry_section.split("## infra").next().unwrap();
        // orders.rs IS imported (by vendor/legacy.rs, even though that
        // importer is out of scope) -- must not look like a false
        // entry point just because its only in-scope neighbor doesn't
        // reveal that.
        assert!(!entry_section.contains("orders.rs"));
        // handlers.rs has no incoming imports at all -- a real entry
        // point, and in scope, so it must still be listed.
        assert!(entry_section.contains("mg_site/handlers.rs"));
    }

    #[test]
    fn top_modules_section_ranks_files_by_import_fan() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let result = run(&qg, &MapQuery::new());
        let joined = result.lines.join("\n");
        assert!(joined.contains("## top modules"));
        assert!(joined.contains("src/lib.rs"));
        assert!(joined.contains("out=1"));
        assert!(joined.contains("src/orders.rs"));
        assert!(joined.contains("in=1"));
    }

    #[test]
    fn entry_points_lists_the_file_with_no_incoming_imports() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let result = run(&qg, &MapQuery::new());
        let joined = result.lines.join("\n");
        assert!(joined.contains("## entry points"));
        assert!(joined.contains("src/lib.rs"));
        // orders.rs *is* imported, so it should not appear as an entry
        // point candidate (only as a fan-in row in the modules section).
        let entry_section = joined.split("## entry points").nth(1).unwrap();
        let entry_section = entry_section.split("## infra").next().unwrap();
        assert!(!entry_section.contains("src/orders.rs"));
    }

    #[test]
    fn infra_and_join_sections_are_explicit_placeholders() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let result = run(&qg, &MapQuery::new());
        let joined = result.lines.join("\n");
        assert!(joined.contains("## infra"));
        assert!(joined.contains("requires M2"));
        assert!(joined.contains("## join"));
        assert!(joined.contains("requires M3"));
    }

    #[test]
    fn budget_is_never_exceeded() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let result = run(
            &qg,
            &MapQuery {
                budget: 3,
                subpath: None,
                sections: None,
                component: None,
            },
        );
        assert!(result.lines.len() <= 3);
        assert!(result.truncation.truncated);
    }

    #[test]
    fn generous_budget_is_not_truncated() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let result = run(&qg, &MapQuery::new());
        assert!(!result.truncation.truncated);
    }

    #[test]
    fn zero_budget_still_terminates_and_suggests_the_default() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let result = run(
            &qg,
            &MapQuery {
                budget: 0,
                subpath: None,
                sections: None,
                component: None,
            },
        );
        assert!(result.lines.is_empty());
        assert!(result.truncation.truncated);
        assert_eq!(
            result.truncation.next_call.as_deref(),
            Some(format!("carto map --budget {}", consts::DEFAULT_MAP_BUDGET)).as_deref()
        );
    }

    #[test]
    fn empty_graph_still_produces_a_well_formed_overview() {
        let qg = QueryGraph::from_document(doc(vec![], vec![]));
        let result = run(&qg, &MapQuery::new());
        assert_eq!(result.counts.files, 0);
        assert!(!result.truncation.truncated);
        let joined = result.lines.join("\n");
        assert!(joined.contains("(none)")); // top modules section, no files
        assert!(joined.contains("(none found)")); // entry points section
    }

    #[test]
    fn section_filter_renders_only_the_requested_section() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let mut sections = BTreeSet::new();
        sections.insert(MapSection::Counts);
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: None,
                sections: Some(sections),
                component: None,
            },
        );
        let joined = result.lines.join("\n");
        assert!(joined.contains("files="));
        assert!(!joined.contains("## top modules"));
        assert!(!joined.contains("## entry points"));
        assert!(!joined.contains("## infra"));
    }

    #[test]
    fn section_filter_does_not_affect_the_structured_counts_field() {
        // §2.2: the structured payload stays exact regardless of
        // --section — only which lines render is restricted.
        let qg = QueryGraph::from_document(small_repo_doc());
        let mut sections = BTreeSet::new();
        sections.insert(MapSection::Modules);
        let scoped = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: None,
                sections: Some(sections),
                component: None,
            },
        );
        let full = run(&qg, &MapQuery::new());
        assert_eq!(scoped.counts.files, full.counts.files);
        assert_eq!(scoped.counts.symbols, full.counts.symbols);
    }

    #[test]
    fn no_sections_filter_keeps_todays_full_overview_behavior() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let with_none = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: None,
                sections: None,
                component: None,
            },
        );
        let with_default = run(&qg, &MapQuery::new());
        assert_eq!(with_none.lines, with_default.lines);
    }

    #[test]
    fn truncation_carries_the_section_filter_forward() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let mut sections = BTreeSet::new();
        sections.insert(MapSection::Counts);
        sections.insert(MapSection::Modules);
        let result = run(
            &qg,
            &MapQuery {
                budget: 1,
                subpath: None,
                sections: Some(sections),
                component: None,
            },
        );
        assert!(result.truncation.truncated);
        let next = result.truncation.next_call.unwrap();
        assert!(next.contains("--section counts"));
        assert!(next.contains("--section modules"));
        assert!(!next.contains("entry-points"));
    }

    #[test]
    fn map_section_parse_is_the_exact_inverse_of_as_str_for_every_variant() {
        for section in [
            MapSection::Counts,
            MapSection::Modules,
            MapSection::EntryPoints,
            MapSection::Infra,
            MapSection::Components,
        ] {
            assert_eq!(MapSection::parse(section.as_str()), Some(section));
        }
        assert_eq!(MapSection::parse("not-a-section"), None);
    }

    // --- ADR-0034/0035: the components section -----------------------

    fn component(name: &str, path: &str, kind: &str) -> crate::components::Component {
        crate::components::Component {
            name: name.to_string(),
            path: path.to_string(),
            kind: kind.to_string(),
            depends_on: vec![],
        }
    }

    fn file_with_component(path: &str, component: &str) -> Node {
        let mut n = file(path);
        if let NodeData::File(f) = &mut n.data {
            f.component = Some(component.to_string());
        }
        n
    }

    /// Two Go components, `orders` and `billing`, each with one file;
    /// `orders`'s file imports `billing`'s file — a real cross-component
    /// edge to exercise the summary.
    fn two_component_doc() -> GraphDocument {
        let orders = file_with_component("services/orders/main.go", "orders");
        let billing = file_with_component("services/billing/main.go", "billing");
        let cross_edge = Edge::new(
            EdgeKind::Imports,
            orders.id.clone(),
            billing.id.clone(),
            Confidence::Certain,
            "package-import".to_string(),
        );
        GraphDocument {
            components: vec![
                component("orders", "services/orders", "go"),
                component("billing", "services/billing", "go"),
            ],
            ..doc(vec![orders, billing], vec![cross_edge])
        }
    }

    #[test]
    fn components_section_lists_each_component_with_its_own_counts() {
        let qg = QueryGraph::from_document(two_component_doc());
        let result = run(&qg, &MapQuery::new());
        assert_eq!(result.counts.components.len(), 2);
        let orders = &result.counts.components["orders"];
        assert_eq!(orders.path, "services/orders");
        assert_eq!(orders.kind, "go");
        assert_eq!(orders.files, 1);
        let joined = result.lines.join("\n");
        assert!(joined.contains("## components"));
        assert!(
            joined.contains("orders  path=services/orders kind=go files=1"),
            "{joined}"
        );
        assert!(
            joined.contains("billing  path=services/billing kind=go files=1"),
            "{joined}"
        );
    }

    #[test]
    fn components_section_summarizes_cross_component_edges() {
        let qg = QueryGraph::from_document(two_component_doc());
        let result = run(&qg, &MapQuery::new());
        let joined = result.lines.join("\n");
        assert!(joined.contains("## cross-component edges"));
        assert!(joined.contains("orders -> billing: 1 imports"), "{joined}");
    }

    fn component_with_deps(
        name: &str,
        path: &str,
        kind: &str,
        depends_on: Vec<&str>,
    ) -> crate::components::Component {
        crate::components::Component {
            name: name.to_string(),
            path: path.to_string(),
            kind: kind.to_string(),
            depends_on: depends_on.into_iter().map(str::to_string).collect(),
        }
    }

    #[test]
    fn a_component_row_renders_its_own_declared_dependencies() {
        let mut d = two_component_doc();
        d.components = vec![
            component_with_deps("orders", "services/orders", "go", vec!["billing"]),
            component_with_deps("billing", "services/billing", "go", vec![]),
        ];
        let qg = QueryGraph::from_document(d);
        let result = run(&qg, &MapQuery::new());
        let joined = result.lines.join("\n");
        assert!(
            joined.contains(
                "orders  path=services/orders kind=go files=1 symbols=0 depends_on=billing"
            ),
            "{joined}"
        );
        assert!(
            joined.contains(
                "billing  path=services/billing kind=go files=1 symbols=0 depends_on=(none)"
            ),
            "{joined}"
        );
    }

    #[test]
    fn cross_component_edge_summary_marks_an_undeclared_crossing() {
        // `two_component_doc`'s own components carry no `depends_on` at
        // all, so `orders -> billing` (a real crossing) is undeclared.
        let qg = QueryGraph::from_document(two_component_doc());
        let result = run(&qg, &MapQuery::new());
        let joined = result.lines.join("\n");
        assert!(
            joined.contains("orders -> billing: 1 imports(certain)  [undeclared]"),
            "{joined}"
        );
    }

    #[test]
    fn cross_component_edge_summary_does_not_mark_a_declared_crossing() {
        let mut d = two_component_doc();
        d.components = vec![
            component_with_deps("orders", "services/orders", "go", vec!["billing"]),
            component_with_deps("billing", "services/billing", "go", vec![]),
        ];
        let qg = QueryGraph::from_document(d);
        let result = run(&qg, &MapQuery::new());
        let joined = result.lines.join("\n");
        assert!(
            joined.contains("orders -> billing: 1 imports(certain)")
                && !joined.contains("[undeclared]"),
            "{joined}"
        );
    }

    #[test]
    fn a_repo_with_no_components_still_prints_the_header_and_none() {
        let qg = QueryGraph::from_document(small_repo_doc());
        let result = run(&qg, &MapQuery::new());
        assert!(result.counts.components.is_empty());
        let joined = result.lines.join("\n");
        assert!(joined.contains("## components"));
        let components_section = joined.split("## components").nth(1).unwrap();
        assert!(components_section.trim_start().starts_with("(none)"));
        assert!(!joined.contains("## cross-component edges"));
    }

    #[test]
    fn component_filter_restricts_the_components_section_and_counts() {
        let qg = QueryGraph::from_document(two_component_doc());
        let mut filter = BTreeSet::new();
        filter.insert("orders".to_string());
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: None,
                sections: None,
                component: Some(filter),
            },
        );
        assert_eq!(result.counts.components.len(), 1);
        assert!(result.counts.components.contains_key("orders"));
        let joined = result.lines.join("\n");
        assert!(joined.contains("orders  path="));
        assert!(!joined.contains("billing  path="));
        // The cross-component edge still shows since `orders` (one of
        // its two endpoints) is in scope.
        assert!(joined.contains("orders -> billing"));
    }

    #[test]
    fn component_filter_also_restricts_file_counts_and_top_modules() {
        let qg = QueryGraph::from_document(two_component_doc());
        let mut filter = BTreeSet::new();
        filter.insert("orders".to_string());
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: None,
                sections: None,
                component: Some(filter),
            },
        );
        assert_eq!(result.counts.files, 1);
        let joined = result.lines.join("\n");
        assert!(joined.contains("services/orders/main.go"));
        assert!(!joined.contains("services/billing/main.go  in="));
    }

    /// ADR-0038: `api` and a nested `internal` component (a second
    /// manifest marker inside `api`'s own directory — the same shape
    /// `components::tests::innermost_component_wins_for_nested_markers`
    /// covers at the discovery layer), each with one file.
    fn nested_component_doc() -> GraphDocument {
        let api = file_with_component("services/api/main.go", "api");
        let internal = file_with_component("services/api/internal/x.go", "internal");
        GraphDocument {
            components: vec![
                component("api", "services/api", "go"),
                component("internal", "services/api/internal", "go"),
            ],
            ..doc(vec![api, internal], vec![])
        }
    }

    #[test]
    fn component_filter_seeds_a_component_nested_under_the_filtered_one() {
        let qg = QueryGraph::from_document(nested_component_doc());
        let mut filter = BTreeSet::new();
        filter.insert("api".to_string());
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: None,
                sections: None,
                component: Some(filter),
            },
        );
        assert_eq!(
            result.counts.components.len(),
            2,
            "`--component api` must also seed the nested `internal` component, not just `api` itself: {:?}",
            result.counts.components.keys().collect::<Vec<_>>()
        );
        assert!(result.counts.components.contains_key("api"));
        assert!(result.counts.components.contains_key("internal"));
        assert_eq!(result.counts.files, 2);
    }

    #[test]
    fn component_filter_on_the_nested_component_does_not_seed_its_ancestor() {
        let qg = QueryGraph::from_document(nested_component_doc());
        let mut filter = BTreeSet::new();
        filter.insert("internal".to_string());
        let result = run(
            &qg,
            &MapQuery {
                budget: consts::DEFAULT_MAP_BUDGET,
                subpath: None,
                sections: None,
                component: Some(filter),
            },
        );
        assert_eq!(result.counts.components.len(), 1);
        assert!(result.counts.components.contains_key("internal"));
        assert!(!result.counts.components.contains_key("api"));
        assert_eq!(result.counts.files, 1);
    }

    #[test]
    fn truncation_carries_the_component_filter_forward() {
        let qg = QueryGraph::from_document(two_component_doc());
        let mut filter = BTreeSet::new();
        filter.insert("orders".to_string());
        let result = run(
            &qg,
            &MapQuery {
                budget: 1,
                subpath: None,
                sections: None,
                component: Some(filter),
            },
        );
        assert!(result.truncation.truncated);
        let next = result.truncation.next_call.unwrap();
        assert!(next.contains("--component orders"), "{next}");
    }
}
