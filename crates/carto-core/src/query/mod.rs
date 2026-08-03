//! The read path over an indexed graph (spec §7): [`QueryGraph`] is an
//! in-memory adjacency index built from a loaded [`crate::graph::load`]
//! document; [`find`], [`deps`], [`map`] are the core functions behind
//! `carto where`/`deps`/`map` (spec §7.1's "commands = MCP tools, same
//! core functions" — these are plain functions over `&QueryGraph`
//! returning serde types, so M4's MCP server calls the same code the CLI
//! does).
//!
//! `QueryGraph` is `BTreeMap`-backed, same as [`crate::graph::Graph`],
//! not `petgraph` — this is
//! `docs/adr/0009-query-traversal-stays-on-btreemap.md`'s revisit of
//! `docs/adr/0005-graph-store-sorted-vec-not-petgraph.md`, which named
//! this milestone as the point to decide against real traversal
//! requirements instead of speculatively. The answer is still no: `deps`
//! is a BFS bounded at [`crate::consts::MAX_DEPS_DEPTH`], and carto's
//! ID-addressed edges (§4.3's merge rule, the future SCIP overlay
//! "replacing `inferred` edges by ID") would need a `NodeId <-> NodeIndex`
//! side map to use petgraph's index-addressed edges anyway.

pub mod deps;
pub mod find;
pub mod map;

use crate::graph::{Edge, EdgeId, GraphDocument, Node, NodeData, NodeId, SymbolNode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use deps::{DepEdge, DepsQuery, DepsResult, Hop, NodeSummary, run as deps};
pub use find::{FindQuery, FindResult, ModuleMatch, SymbolMatch, run as find};
pub use map::{MapCounts, MapQuery, MapResult, MapSection, run as map};

/// The spec §7.2 truncation contract every query result ends with:
/// "Every response ends with `truncated: bool` and, if true, the exact
/// follow-up call to get more."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truncation {
    pub truncated: bool,
    pub next_call: Option<String>,
}

impl Truncation {
    pub fn none() -> Self {
        Truncation {
            truncated: false,
            next_call: None,
        }
    }

    pub fn more(next_call: String) -> Self {
        Truncation {
            truncated: true,
            next_call: Some(next_call),
        }
    }
}

/// Which direction to traverse an edge from a node: `out` follows edges
/// where the node is `from` (spec §7.1's `deps --dir`: "what this depends
/// on"), `in` follows edges where it's `to`, `both` follows either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    In,
    Out,
    Both,
}

impl Direction {
    /// Matches the `--dir` flag spelling (spec §7.1) and this enum's own
    /// serde spelling — used when building a `Truncation::next_call`
    /// string, so the suggested follow-up command is copy-pasteable.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
            Direction::Both => "both",
        }
    }
}

/// An in-memory, queryable view of a loaded `graph.json` (spec §4.4).
/// Built once per command invocation from [`crate::graph::load`]'s
/// output; read-only — nothing in the query layer mutates or persists a
/// graph.
pub struct QueryGraph {
    nodes: BTreeMap<NodeId, Node>,
    edges: BTreeMap<EdgeId, Edge>,
    /// `NodeId` -> outgoing edge IDs, in ID-sorted order (built by
    /// iterating `edges`, itself `BTreeMap`-sorted, so no separate sort
    /// step — the same pattern `Graph::into_sorted_parts` uses on the
    /// write side).
    out_edges: BTreeMap<NodeId, Vec<EdgeId>>,
    /// `NodeId` -> incoming edge IDs, same ordering guarantee.
    in_edges: BTreeMap<NodeId, Vec<EdgeId>>,
}

impl QueryGraph {
    pub fn from_document(doc: GraphDocument) -> Self {
        let edges: BTreeMap<EdgeId, Edge> =
            doc.edges.into_iter().map(|e| (e.id.clone(), e)).collect();

        let mut out_edges: BTreeMap<NodeId, Vec<EdgeId>> = BTreeMap::new();
        let mut in_edges: BTreeMap<NodeId, Vec<EdgeId>> = BTreeMap::new();
        for edge in edges.values() {
            out_edges
                .entry(edge.from.clone())
                .or_default()
                .push(edge.id.clone());
            in_edges
                .entry(edge.to.clone())
                .or_default()
                .push(edge.id.clone());
        }

        let nodes: BTreeMap<NodeId, Node> =
            doc.nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

        QueryGraph {
            nodes,
            edges,
            out_edges,
            in_edges,
        }
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn edge(&self, id: &EdgeId) -> Option<&Edge> {
        self.edges.get(id)
    }

    /// All nodes, in ID-sorted order (`BTreeMap` iteration).
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// All edges, in ID-sorted order (`BTreeMap` iteration). Used by
    /// `map`'s repo-wide scans (counts, fan-in/out ranking), which need
    /// every edge once rather than per-node neighbor lookups.
    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.values()
    }

    /// Edges touching `id` in the given direction, ID-sorted.
    pub fn neighbors(&self, id: &NodeId, dir: Direction) -> Vec<&Edge> {
        let mut result = Vec::new();
        if matches!(dir, Direction::Out | Direction::Both) {
            if let Some(ids) = self.out_edges.get(id) {
                result.extend(ids.iter().filter_map(|eid| self.edges.get(eid)));
            }
        }
        if matches!(dir, Direction::In | Direction::Both) {
            if let Some(ids) = self.in_edges.get(id) {
                result.extend(ids.iter().filter_map(|eid| self.edges.get(eid)));
            }
        }
        result
    }

    /// Renders a symbol's location as `path:start-end` (spec §7.2: "All
    /// locations `path:start-end`"), resolving `sym.file` through the
    /// node table. Falls back to a placeholder rather than panicking if
    /// the file node is somehow missing (e.g. a hand-edited graph.json) —
    /// query code must stay side-effect-free and non-panicking on
    /// malformed-but-parseable input.
    pub fn location(&self, sym: &SymbolNode) -> String {
        let file_path = self
            .node(&sym.file)
            .and_then(|n| n.data.as_file())
            .map(|f| f.path.as_str())
            .unwrap_or("<unknown-file>");
        format!("{file_path}:{}-{}", sym.start_line, sym.end_line)
    }

    /// Whether `id` is "in scope" for a `--subpath <dir>` filter (spec
    /// §7.1), shared by `find`/`deps`/`map` rather than three bespoke
    /// implementations. `subpath: None` (or empty/whitespace-only,
    /// tolerated as equivalent) means "no filter" — always `true`. A
    /// `File` node is in scope iff its `path` equals `subpath` or
    /// starts with `subpath` + `"/"` — segment-boundary-safe, so
    /// `"src/handlers"` doesn't also match `"src/handlers2/x.ts"`. A
    /// `Symbol` node is in scope iff its *owning file* is (a dangling
    /// file reference is conservatively out of scope, not a panic). A
    /// `Module` node is always in scope: packages have no directory, so
    /// subpath filtering was never meant to exclude them by path —
    /// they're either pulled in by an in-scope file's own edges or
    /// they aren't shown at all, the same way they already work today.
    pub fn path_in_scope(&self, id: &NodeId, subpath: Option<&str>) -> bool {
        let prefix = match subpath.map(str::trim) {
            None | Some("") => return true,
            Some(p) => p,
        };
        let path = match self.node(id).map(|n| &n.data) {
            Some(NodeData::File(f)) => f.path.as_str(),
            Some(NodeData::Symbol(s)) => match self.node(&s.file).and_then(|n| n.data.as_file()) {
                Some(f) => f.path.as_str(),
                None => return false,
            },
            Some(NodeData::Module(_)) => return true,
            None => return false,
        };
        path == prefix || path.starts_with(&format!("{prefix}/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Confidence, EdgeKind, FileNode, SymKind, SymbolNode};
    use crate::lang::Lang;
    use crate::taint::Provenance;

    fn file_node(path: &str) -> Node {
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

    fn symbol_node(relpath: &str, name: &str, file: &NodeId) -> Node {
        Node::symbol(
            crate::graph::sym_id(relpath, "function", name, 1),
            Provenance::Syntactic,
            "test@1",
            SymbolNode {
                name: name.to_string(),
                sym_kind: SymKind::Function,
                file: file.clone(),
                start_line: 1,
                end_line: 3,
                signature: None,
                unresolved_calls: vec![],
                uncaptured_inbound_calls: 0,
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

    #[test]
    fn neighbors_are_symmetric_across_direction() {
        let a = crate::graph::file_id("a.rs");
        let b = crate::graph::file_id("b.rs");
        let edge = Edge::new(
            EdgeKind::Imports,
            a.clone(),
            b.clone(),
            Confidence::Certain,
            "mod-declaration".to_string(),
        );
        let qg = QueryGraph::from_document(doc(
            vec![file_node("a.rs"), file_node("b.rs")],
            vec![edge.clone()],
        ));

        let out = qg.neighbors(&a, Direction::Out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, edge.id);

        let inn = qg.neighbors(&b, Direction::In);
        assert_eq!(inn.len(), 1);
        assert_eq!(inn[0].id, edge.id);

        assert!(qg.neighbors(&a, Direction::In).is_empty());
        assert!(qg.neighbors(&b, Direction::Out).is_empty());

        assert_eq!(qg.neighbors(&a, Direction::Both).len(), 1);
    }

    #[test]
    fn location_formats_as_path_colon_start_dash_end() {
        let file = crate::graph::file_id("src/orders.rs");
        let sym = symbol_node("src/orders.rs", "parse_order", &file);
        let SymbolNode {
            file: sym_file,
            start_line,
            end_line,
            ..
        } = match &sym.data {
            crate::graph::NodeData::Symbol(s) => s.clone(),
            _ => unreachable!(),
        };
        let qg = QueryGraph::from_document(doc(vec![file_node("src/orders.rs")], vec![]));
        let rendered = qg.location(&SymbolNode {
            name: "parse_order".to_string(),
            sym_kind: SymKind::Function,
            file: sym_file,
            start_line,
            end_line,
            signature: None,
            unresolved_calls: vec![],
            uncaptured_inbound_calls: 0,
        });
        assert_eq!(rendered, "src/orders.rs:1-3");
    }

    #[test]
    fn location_falls_back_when_file_node_missing() {
        let qg = QueryGraph::from_document(doc(vec![], vec![]));
        let rendered = qg.location(&SymbolNode {
            name: "orphan".to_string(),
            sym_kind: SymKind::Function,
            file: crate::graph::file_id("nowhere.rs"),
            start_line: 1,
            end_line: 1,
            signature: None,
            unresolved_calls: vec![],
            uncaptured_inbound_calls: 0,
        });
        assert_eq!(rendered, "<unknown-file>:1-1");
    }
}
