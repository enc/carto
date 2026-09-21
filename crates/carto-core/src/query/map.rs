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
use crate::graph::{EdgeKind, NodeData, NodeId};
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
        }
    }

    /// Inverse of [`as_str`](Self::as_str). `None` for anything else.
    pub fn parse(s: &str) -> Option<MapSection> {
        match s {
            "counts" => Some(MapSection::Counts),
            "modules" => Some(MapSection::Modules),
            "entry-points" => Some(MapSection::EntryPoints),
            "infra" => Some(MapSection::Infra),
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
}

impl MapQuery {
    pub fn new() -> Self {
        MapQuery {
            budget: consts::DEFAULT_MAP_BUDGET,
            subpath: None,
            sections: None,
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
    let counts = compute_counts(qg, subpath);

    // `counts` (the structured field) stays exact and whole regardless
    // of `--section` (§2.2: the machine payload is never trimmed, only
    // `--section` lets a caller ask for less of the rendered text) —
    // this filter only decides which of these four blocks contribute to
    // `lines`.
    let all_sections: Vec<(MapSection, Vec<String>)> = vec![
        (MapSection::Counts, counts_lines(&counts)),
        (MapSection::Modules, top_modules_lines(qg, subpath)),
        (MapSection::EntryPoints, entry_points_lines(qg, subpath)),
        (MapSection::Infra, infra_lines()),
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
        Truncation::more(format!("carto map --budget {next_budget}{section_flags}"))
    } else {
        Truncation::none()
    };

    MapResult {
        counts,
        lines,
        truncation,
    }
}

fn compute_counts(qg: &QueryGraph, subpath: Option<&str>) -> MapCounts {
    let mut files = 0;
    let mut symbols = 0;
    for node in qg.nodes() {
        if !qg.path_in_scope(&node.id, subpath) {
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
        let from_in = qg.path_in_scope(&edge.from, subpath);
        let to_in = qg.path_in_scope(&edge.to, subpath);
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
    }
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
fn top_modules_lines(qg: &QueryGraph, subpath: Option<&str>) -> Vec<String> {
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
        // not a whole-repo number (ADR-0014).
        if qg.path_in_scope(&edge.from, subpath) {
            if let Some(entry) = module_fan_in.get_mut(&edge.to) {
                *entry += 1;
            }
        }
    }

    let mut lines = vec!["## top modules (imports fan-in/out)".to_string()];

    let mut ranked_files: Vec<(String, usize, usize)> = file_fan
        .into_iter()
        .filter(|(id, (inn, out))| (*inn > 0 || *out > 0) && qg.path_in_scope(id, subpath))
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
fn entry_points_lines(qg: &QueryGraph, subpath: Option<&str>) -> Vec<String> {
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
            if !has_incoming_import.contains(&node.id) && qg.path_in_scope(&node.id, subpath) {
                candidates.push(f.path.clone());
            }
        }
    }
    candidates.sort();

    let mut main_fns: Vec<String> = Vec::new();
    for node in qg.nodes() {
        if let NodeData::Symbol(s) = &node.data {
            if s.name == "main" && qg.path_in_scope(&node.id, subpath) {
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

/// Explicit placeholders rather than omitting the sections entirely —
/// an absent section reads as "no infra found"; a present one saying
/// "not implemented" is honest about why (spec §10: infra is M2, the
/// join is M3).
fn infra_lines() -> Vec<String> {
    vec![
        "## infra".to_string(),
        "  none — requires M2 (infrastructure graph)".to_string(),
        "## join".to_string(),
        "  none — requires M3 (code<->infra join)".to_string(),
    ]
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
        ] {
            assert_eq!(MapSection::parse(section.as_str()), Some(section));
        }
        assert_eq!(MapSection::parse("not-a-section"), None);
    }
}
