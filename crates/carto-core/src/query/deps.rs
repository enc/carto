//! `carto deps <id|name>` (spec §7.1: `--dir in|out|both`, `--depth N`
//! (≤5), `--kinds` -> "adjacency listing with confidence"). A BFS bounded
//! at [`crate::consts::MAX_DEPS_DEPTH`], reusing [`super::find`] (exact
//! match) to resolve a bare symbol name to a [`NodeId`] when the
//! argument isn't already one.
//!
//! Reports a **spanning-tree view**, not every edge in the reachable
//! subgraph: each node is listed once, via the edge that first reached
//! it (shallowest depth wins — the BFS visited-set property). An edge
//! connecting two nodes that were both already discovered earlier (a
//! "cross edge" in BFS terms) isn't listed separately. Spec §7.1 asks
//! for "an adjacency listing," which this satisfies; listing every
//! induced-subgraph edge as well is a real broadening left for if a
//! concrete need shows up, not an oversight.

use super::{Direction, FindQuery, QueryGraph, Truncation, find};
use crate::consts;
use crate::error::{Error, ErrorKind, Result};
use crate::graph::{Confidence, EdgeKind, NodeData, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct DepsQuery {
    pub target: String,
    pub dir: Direction,
    pub depth: u32,
    /// `None` means "no filter" (every edge kind). `Some(set)` keeps
    /// only edges whose kind is in the set.
    pub kinds: Option<BTreeSet<EdgeKind>>,
}

impl DepsQuery {
    pub fn new(target: impl Into<String>) -> Self {
        DepsQuery {
            target: target.into(),
            dir: Direction::Out,
            depth: 1,
            kinds: None,
        }
    }
}

/// A compact reference to a node, used both for `deps`'s `root` and for
/// each hop's endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSummary {
    pub id: NodeId,
    /// Matches [`NodeData::kind_str`] ("file"/"symbol"/"module").
    pub kind: String,
    /// The symbol name, file path, or module path — whichever the node
    /// kind has.
    pub label: String,
    /// `path:start-end` (spec §7.2), only present for `Symbol` nodes.
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepEdge {
    pub kind: EdgeKind,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
    /// Which way this edge actually points relative to the node that
    /// discovered it — meaningful when `dir` is `both`, where a single
    /// hop can contain edges pointing either direction.
    pub direction: Direction,
    pub node: NodeSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hop {
    pub depth: u32,
    pub edges: Vec<DepEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepsResult {
    pub root: NodeSummary,
    pub hops: Vec<Hop>,
    pub truncation: Truncation,
}

pub fn run(qg: &QueryGraph, query: &DepsQuery) -> Result<DepsResult> {
    if query.depth > consts::MAX_DEPS_DEPTH {
        return Err(Error::new(
            ErrorKind::UserError,
            format!(
                "--depth {} exceeds the cap of {}",
                query.depth,
                consts::MAX_DEPS_DEPTH
            ),
        ));
    }

    let root_id = resolve_target(qg, &query.target)?;
    let root = summarize(qg, &root_id);

    let mut visited: BTreeSet<NodeId> = BTreeSet::new();
    visited.insert(root_id.clone());
    let mut frontier: Vec<NodeId> = vec![root_id];
    let mut hops: Vec<Hop> = Vec::new();
    let mut hit_depth_cap = false;

    for depth in 1..=query.depth {
        let mut edges_this_hop: Vec<DepEdge> = Vec::new();
        let mut next_frontier: Vec<NodeId> = Vec::new();

        for node_id in &frontier {
            for edge in qg.neighbors(node_id, query.dir) {
                if let Some(kinds) = &query.kinds {
                    if !kinds.contains(&edge.kind) {
                        continue;
                    }
                }
                let (other, direction) = if &edge.from == node_id {
                    (edge.to.clone(), Direction::Out)
                } else {
                    (edge.from.clone(), Direction::In)
                };
                // Shallowest depth wins: a node already visited (whether
                // from an earlier hop or an earlier edge in this same
                // hop) isn't reported again.
                if !visited.insert(other.clone()) {
                    continue;
                }
                edges_this_hop.push(DepEdge {
                    kind: edge.kind,
                    confidence: edge.confidence,
                    evidence: edge.evidence.clone(),
                    direction,
                    node: summarize(qg, &other),
                });
                next_frontier.push(other);
            }
        }

        if edges_this_hop.is_empty() {
            break;
        }

        hops.push(Hop {
            depth,
            edges: edges_this_hop,
        });
        frontier = next_frontier;

        if depth == query.depth && !frontier.is_empty() {
            hit_depth_cap = true;
        }
    }

    let truncation = match (hit_depth_cap, query.depth < consts::MAX_DEPS_DEPTH) {
        (true, true) => Truncation::more(format!(
            "carto deps {} --dir {} --depth {}",
            query.target,
            query.dir.as_str(),
            query.depth + 1
        )),
        (true, false) => Truncation {
            truncated: true,
            next_call: None,
        },
        (false, _) => Truncation::none(),
    };

    Ok(DepsResult {
        root,
        hops,
        truncation,
    })
}

/// Resolves `target` to a [`NodeId`]: used as-is if it's already an
/// existing node's ID; otherwise looked up as an exact symbol name via
/// [`super::find`]. Zero matches or more than one match are both
/// `UserError`s (INV-8's honesty rule, applied to the CLI surface: no
/// silent pick among ambiguous candidates) — the multi-match case lists
/// every candidate's ID so the caller can re-run with one.
fn resolve_target(qg: &QueryGraph, target: &str) -> Result<NodeId> {
    if let Some(node) = qg.nodes().find(|n| n.id.as_str() == target) {
        return Ok(node.id.clone());
    }

    let matches = find(
        qg,
        &FindQuery {
            needle: target.to_string(),
            exact: true,
            limit: usize::MAX,
        },
    )
    .matches;

    match matches.len() {
        0 => Err(Error::new(
            ErrorKind::UserError,
            format!("no node ID or symbol named `{target}` found in this index"),
        )),
        1 => Ok(matches.into_iter().next().unwrap().id),
        _ => {
            let candidates: Vec<String> = matches
                .iter()
                .map(|m| format!("{} ({})", m.id, m.location))
                .collect();
            Err(Error::new(
                ErrorKind::UserError,
                format!(
                    "`{target}` matches multiple symbols; specify one by ID: {}",
                    candidates.join(", ")
                ),
            ))
        }
    }
}

fn summarize(qg: &QueryGraph, id: &NodeId) -> NodeSummary {
    match qg.node(id).map(|n| &n.data) {
        Some(data @ NodeData::File(f)) => NodeSummary {
            id: id.clone(),
            kind: data.kind_str().to_string(),
            label: f.path.clone(),
            location: None,
        },
        Some(data @ NodeData::Symbol(s)) => NodeSummary {
            id: id.clone(),
            kind: data.kind_str().to_string(),
            label: s.name.clone(),
            location: Some(qg.location(s)),
        },
        Some(data @ NodeData::Module(m)) => NodeSummary {
            id: id.clone(),
            kind: data.kind_str().to_string(),
            label: m.path.clone(),
            location: None,
        },
        // A dangling reference would mean the graph itself is malformed
        // (persist's own `validate_edge_endpoints` should prevent this at
        // write time) — stay non-panicking on read regardless.
        None => NodeSummary {
            id: id.clone(),
            kind: "unknown".to_string(),
            label: "<missing>".to_string(),
            location: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Edge, FileNode, GraphDocument, Node, SymKind, SymbolNode};
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

    /// `handle -> parse_order -> validate` (a two-hop chain) plus an
    /// unrelated `noise` symbol, matching the shape of `calls` edges
    /// carto's own resolver produces.
    fn chain_doc() -> GraphDocument {
        let f = file("src/lib.rs");
        let handle = symbol("src/lib.rs", "handle", &f.id, 1);
        let parse_order = symbol("src/lib.rs", "parse_order", &f.id, 10);
        let validate = symbol("src/lib.rs", "validate", &f.id, 20);
        let noise = symbol("src/lib.rs", "noise", &f.id, 30);
        let e1 = Edge::new(
            EdgeKind::Calls,
            handle.id.clone(),
            parse_order.id.clone(),
            Confidence::Inferred,
            "same-file".to_string(),
        );
        let e2 = Edge::new(
            EdgeKind::Calls,
            parse_order.id.clone(),
            validate.id.clone(),
            Confidence::Inferred,
            "same-file".to_string(),
        );
        doc(vec![f, handle, parse_order, validate, noise], vec![e1, e2])
    }

    #[test]
    fn out_direction_follows_calls_forward() {
        let qg = QueryGraph::from_document(chain_doc());
        let query = DepsQuery::new("handle");
        let result = run(&qg, &query).unwrap();
        assert_eq!(result.root.label, "handle");
        assert_eq!(result.hops.len(), 1);
        assert_eq!(result.hops[0].edges.len(), 1);
        assert_eq!(result.hops[0].edges[0].node.label, "parse_order");
        assert_eq!(result.hops[0].edges[0].direction, Direction::Out);
    }

    #[test]
    fn in_direction_follows_calls_backward() {
        let qg = QueryGraph::from_document(chain_doc());
        let query = DepsQuery {
            target: "parse_order".to_string(),
            dir: Direction::In,
            depth: 1,
            kinds: None,
        };
        let result = run(&qg, &query).unwrap();
        assert_eq!(result.hops[0].edges.len(), 1);
        assert_eq!(result.hops[0].edges[0].node.label, "handle");
        assert_eq!(result.hops[0].edges[0].direction, Direction::In);
    }

    #[test]
    fn depth_bounds_how_far_the_bfs_travels() {
        let qg = QueryGraph::from_document(chain_doc());

        let shallow = run(&qg, &DepsQuery::new("handle")).unwrap();
        assert_eq!(shallow.hops.len(), 1); // depth defaults to 1: only parse_order

        let deep = run(
            &qg,
            &DepsQuery {
                target: "handle".to_string(),
                dir: Direction::Out,
                depth: 2,
                kinds: None,
            },
        )
        .unwrap();
        assert_eq!(deep.hops.len(), 2);
        assert_eq!(deep.hops[1].edges[0].node.label, "validate");
    }

    #[test]
    fn depth_above_cap_is_a_user_error() {
        let qg = QueryGraph::from_document(chain_doc());
        let query = DepsQuery {
            target: "handle".to_string(),
            dir: Direction::Out,
            depth: consts::MAX_DEPS_DEPTH + 1,
            kinds: None,
        };
        let err = run(&qg, &query).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UserError);
    }

    #[test]
    fn cycle_terminates_via_visited_set() {
        let f = file("src/lib.rs");
        let a = symbol("src/lib.rs", "a", &f.id, 1);
        let b = symbol("src/lib.rs", "b", &f.id, 10);
        let e1 = Edge::new(
            EdgeKind::Calls,
            a.id.clone(),
            b.id.clone(),
            Confidence::Inferred,
            "same-file".to_string(),
        );
        let e2 = Edge::new(
            EdgeKind::Calls,
            b.id.clone(),
            a.id.clone(),
            Confidence::Inferred,
            "same-file".to_string(),
        );
        let qg = QueryGraph::from_document(doc(vec![f, a, b], vec![e1, e2]));

        let query = DepsQuery {
            target: "a".to_string(),
            dir: Direction::Out,
            depth: consts::MAX_DEPS_DEPTH,
            kinds: None,
        };
        // Must terminate (this test would hang forever on an unbounded
        // cyclic BFS with no visited set) and report exactly one hop:
        // a -> b, then b -> a is skipped since a is already visited.
        let result = run(&qg, &query).unwrap();
        assert_eq!(result.hops.len(), 1);
    }

    #[test]
    fn kinds_filter_excludes_other_edge_kinds() {
        let f = file("src/lib.rs");
        let a = symbol("src/lib.rs", "a", &f.id, 1);
        let b = symbol("src/lib.rs", "b", &f.id, 10);
        let calls = Edge::new(
            EdgeKind::Calls,
            a.id.clone(),
            b.id.clone(),
            Confidence::Inferred,
            "same-file".to_string(),
        );
        let contains = Edge::new(
            EdgeKind::Contains,
            f.id.clone(),
            a.id.clone(),
            Confidence::Certain,
            "extractor:rust".to_string(),
        );
        let qg = QueryGraph::from_document(doc(vec![f, a, b], vec![calls, contains]));

        let mut kinds = BTreeSet::new();
        kinds.insert(EdgeKind::Contains);
        let query = DepsQuery {
            target: "a".to_string(),
            dir: Direction::Both,
            depth: 1,
            kinds: Some(kinds),
        };
        let result = run(&qg, &query).unwrap();
        assert_eq!(result.hops[0].edges.len(), 1);
        assert_eq!(result.hops[0].edges[0].kind, EdgeKind::Contains);
    }

    #[test]
    fn ambiguous_name_is_a_user_error_listing_candidates() {
        let f = file("src/lib.rs");
        let a = symbol("src/a.rs", "dup", &f.id, 1);
        let b = symbol("src/b.rs", "dup", &f.id, 1);
        let qg = QueryGraph::from_document(doc(vec![f, a, b], vec![]));

        let err = run(&qg, &DepsQuery::new("dup")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UserError);
        assert!(format!("{err}").contains("multiple"));
    }

    #[test]
    fn unknown_target_is_a_user_error() {
        let qg = QueryGraph::from_document(doc(vec![], vec![]));
        let err = run(&qg, &DepsQuery::new("nonexistent")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UserError);
    }

    #[test]
    fn target_can_be_a_raw_node_id() {
        let qg = QueryGraph::from_document(chain_doc());
        let handle_id = crate::graph::sym_id("src/lib.rs", "function", "handle", 1);
        let query = DepsQuery::new(handle_id.as_str());
        let result = run(&qg, &query).unwrap();
        assert_eq!(result.root.label, "handle");
    }

    #[test]
    fn confidence_and_evidence_are_surfaced_on_every_row() {
        let qg = QueryGraph::from_document(chain_doc());
        let result = run(&qg, &DepsQuery::new("handle")).unwrap();
        assert_eq!(result.hops[0].edges[0].confidence, Confidence::Inferred);
        assert_eq!(result.hops[0].edges[0].evidence, vec!["same-file"]);
    }

    #[test]
    fn truncation_suggests_a_deeper_depth_when_the_cap_stopped_traversal() {
        let qg = QueryGraph::from_document(chain_doc());
        let query = DepsQuery {
            target: "handle".to_string(),
            dir: Direction::Out,
            depth: 1,
            kinds: None,
        };
        let result = run(&qg, &query).unwrap();
        assert!(result.truncation.truncated);
        assert_eq!(
            result.truncation.next_call.as_deref(),
            Some("carto deps handle --dir out --depth 2")
        );
    }

    #[test]
    fn no_truncation_when_the_frontier_is_exhausted_before_the_depth_cap() {
        let qg = QueryGraph::from_document(chain_doc());
        let query = DepsQuery {
            target: "handle".to_string(),
            dir: Direction::Out,
            depth: consts::MAX_DEPS_DEPTH,
            kinds: None,
        };
        let result = run(&qg, &query).unwrap();
        assert!(!result.truncation.truncated);
    }
}
