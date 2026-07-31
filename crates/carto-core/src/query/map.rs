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

#[derive(Debug, Clone)]
pub struct MapQuery {
    pub budget: u32,
}

impl MapQuery {
    pub fn new() -> Self {
        MapQuery {
            budget: consts::DEFAULT_MAP_BUDGET,
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
    let counts = compute_counts(qg);

    let sections: Vec<Vec<String>> = vec![
        counts_lines(&counts),
        top_modules_lines(qg),
        entry_points_lines(qg),
        infra_lines(),
    ];

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
        Truncation::more(format!("carto map --budget {next_budget}"))
    } else {
        Truncation::none()
    };

    MapResult {
        counts,
        lines,
        truncation,
    }
}

fn compute_counts(qg: &QueryGraph) -> MapCounts {
    let mut files = 0;
    let mut symbols = 0;
    let mut modules = 0;
    for node in qg.nodes() {
        match &node.data {
            NodeData::File(_) => files += 1,
            NodeData::Symbol(_) => symbols += 1,
            NodeData::Module(_) => modules += 1,
        }
    }

    let mut edges_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for edge in qg.edges() {
        *edges_by_kind
            .entry(edge.kind.as_str().to_string())
            .or_insert(0) += 1;
    }

    MapCounts {
        files,
        symbols,
        modules,
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
fn top_modules_lines(qg: &QueryGraph) -> Vec<String> {
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
        if let Some(entry) = file_fan.get_mut(&edge.from) {
            entry.1 += 1;
        }
        if let Some(entry) = file_fan.get_mut(&edge.to) {
            entry.0 += 1;
        }
        if let Some(entry) = module_fan_in.get_mut(&edge.to) {
            *entry += 1;
        }
    }

    let mut lines = vec!["## top modules (imports fan-in/out)".to_string()];

    let mut ranked_files: Vec<(String, usize, usize)> = file_fan
        .into_iter()
        .filter(|(_, (inn, out))| *inn > 0 || *out > 0)
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
fn entry_points_lines(qg: &QueryGraph) -> Vec<String> {
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
            if !has_incoming_import.contains(&node.id) {
                candidates.push(f.path.clone());
            }
        }
    }
    candidates.sort();

    let mut main_fns: Vec<String> = Vec::new();
    for node in qg.nodes() {
        if let NodeData::Symbol(s) = &node.data {
            if s.name == "main" {
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
        let result = run(&qg, &MapQuery { budget: 3 });
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
        let result = run(&qg, &MapQuery { budget: 0 });
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
}
