//! The graph store (spec §3.1, §4). In-memory construction
//! ([`Graph`]) plus the on-disk shape and serialisation choke-point
//! ([`persist`]).
//!
//! Backed by `BTreeMap<NodeId, Node>` / `BTreeMap<EdgeId, Edge>` rather
//! than `petgraph` this slice — see
//! `docs/adr/0005-graph-store-sorted-vec-not-petgraph.md`. `BTreeMap`
//! iteration is already sorted by key, which is exactly spec §4.4's
//! on-disk requirement ("nodes: [ … sorted by id … ]"), so no separate
//! sort step is needed at persist time.

pub mod edge;
pub mod id;
pub mod load;
pub mod node;
pub mod persist;

pub use edge::{Confidence, Edge, EdgeKind, MAX_EVIDENCE_ENTRIES};
pub use id::{EdgeId, NodeId, contract_id, edge_id, file_id, module_id, sym_id};
pub use load::load;
pub use node::{
    ContractNode, ExclusionReason, FileNode, InboundCallSite, ModuleNode, Node, NodeData,
    SkipReason, SymKind, SymbolNode, UnresolvedCall,
};
pub use persist::{GraphDocument, Manifest, PersistMeta, persist};

use std::collections::BTreeMap;

/// In-memory graph under construction. Producers (`walk`, and later the
/// language extractors) insert into this; [`persist`] consumes it.
#[derive(Debug, Default)]
pub struct Graph {
    nodes: BTreeMap<NodeId, Node>,
    edges: BTreeMap<EdgeId, Edge>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a node. The spec defines merge semantics for *edges*
    /// (§4.3) but not nodes — a colliding node ID is a producer bug (e.g.
    /// two `File` nodes for the same path), not a case with defined
    /// behavior, so this simply overwrites.
    pub fn insert_node(&mut self, node: Node) {
        self.nodes.insert(node.id.clone(), node);
    }

    /// Inserts an edge, merging into an existing same-ID edge per the
    /// spec §4.3 rule (see [`Edge::merge`]).
    pub fn insert_edge(&mut self, edge: Edge) {
        match self.edges.get_mut(&edge.id) {
            Some(existing) => existing.merge(edge),
            None => {
                self.edges.insert(edge.id.clone(), edge);
            }
        }
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn contains_node(&self, id: &NodeId) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Mutable access to every node, in no particular order. The one
    /// in-place mutation path in the whole crate — added for
    /// [`crate::redact::redact`] (spec §6.5: redact runs before
    /// [`into_sorted_parts`](Self::into_sorted_parts), so it needs to
    /// rewrite `TaintedString` fields on the `Graph` itself, not on the
    /// already-extracted `Vec<Node>`). Every other consumer still only
    /// builds a `Graph` via [`insert_node`](Self::insert_node) and reads
    /// it via `into_sorted_parts`; order doesn't matter here since
    /// redaction is a per-node, order-independent rewrite.
    pub fn nodes_mut(&mut self) -> impl Iterator<Item = &mut Node> {
        self.nodes.values_mut()
    }

    /// Consumes the graph, returning its nodes and edges in ID-sorted
    /// order (spec §4.4). This is the only way out of a `Graph` — callers
    /// (i.e. [`persist`]) get the sort guarantee without a separate sort
    /// call by construction.
    pub fn into_sorted_parts(self) -> (Vec<Node>, Vec<Edge>) {
        (
            self.nodes.into_values().collect(),
            self.edges.into_values().collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taint::Provenance;

    fn file_node(path: &str) -> Node {
        Node::file(
            id::file_id(path),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: path.to_string(),
                lang: crate::lang::Lang::PlainText,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
            },
        )
    }

    #[test]
    fn nodes_come_out_sorted_by_id_regardless_of_insertion_order() {
        let mut g = Graph::new();
        g.insert_node(file_node("zzz.rs"));
        g.insert_node(file_node("aaa.rs"));
        g.insert_node(file_node("mmm.rs"));

        let (nodes, _) = g.into_sorted_parts();
        let ids: Vec<&NodeId> = nodes.iter().map(|n| &n.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn duplicate_edge_id_merges_instead_of_duplicating() {
        let mut g = Graph::new();
        let a = id::file_id("a.rs");
        let b = id::file_id("b.rs");
        g.insert_edge(Edge::new(
            EdgeKind::Imports,
            a.clone(),
            b.clone(),
            Confidence::Inferred,
            "first".into(),
        ));
        g.insert_edge(Edge::new(
            EdgeKind::Imports,
            a,
            b,
            Confidence::Certain,
            "second".into(),
        ));
        assert_eq!(g.edge_count(), 1);
        let (_, edges) = g.into_sorted_parts();
        assert_eq!(edges[0].confidence, Confidence::Certain);
        assert_eq!(edges[0].evidence.len(), 2);
    }
}
