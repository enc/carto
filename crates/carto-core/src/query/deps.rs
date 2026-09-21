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
use crate::graph::{Confidence, EdgeKind, InboundCallSite, NodeData, NodeId, UnresolvedCall};
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
    /// Restrict *reported* rows to nodes under this repo-relative
    /// directory (spec §7.1's `--subpath`) — see
    /// [`QueryGraph::path_in_scope`]. The BFS traversal itself is
    /// unaffected: it still expands through out-of-scope nodes, so a
    /// dependency chain that dips outside the subtree and back is
    /// never silently broken; only which discovered nodes get shown is
    /// filtered. Does *not* restrict `target`-by-name resolution unless
    /// the name is already ambiguous without it — the deps root is very
    /// commonly outside the subtree being reported on, so scoping it
    /// unconditionally would break that common case (see
    /// `resolve_target`). `None` means no restriction.
    pub subpath: Option<String>,
}

impl DepsQuery {
    pub fn new(target: impl Into<String>) -> Self {
        DepsQuery {
            target: target.into(),
            dir: Direction::Out,
            depth: 1,
            kinds: None,
            subpath: None,
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
    /// Whether this edge's node lives in the same file as `root`
    /// (ADR-0021, §1.4) — `false` for a `Module` node (which has no
    /// file) or when `root` itself is a `Module`. Motivated by T5: a
    /// caller in the same file as the symbol it calls was misread as
    /// "the method definition itself" rather than a distinct call site,
    /// a legibility gap this flag exists to close without changing the
    /// data model.
    pub same_file_as_root: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hop {
    pub depth: u32,
    pub edges: Vec<DepEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepsResult {
    pub root: NodeSummary,
    /// The root symbol's own `unresolved_calls` (empty for a File/Module
    /// root, or a Symbol root with none) — surfaced here because `--dir
    /// out` returning zero edges is otherwise indistinguishable from
    /// "this symbol genuinely calls nothing" vs. "every call it makes
    /// landed in `unresolved_calls`" (ambiguous candidates, or calls
    /// into code this repo never indexed — INV-8's honest-omission
    /// behavior, previously invisible from `deps`'s output). Root-only,
    /// not per-hop: answers the reported pain point without bloating
    /// every row of a possibly-large traversal; broaden only if a
    /// concrete need shows up.
    pub root_unresolved_calls: Vec<UnresolvedCall>,
    /// The root symbol's own [`crate::graph::SymbolNode::
    /// uncaptured_inbound_calls`] (0 for a File/Module root, or a
    /// language with no such exclusion) — surfaced here for the same
    /// reason as `root_unresolved_calls`, but for the *opposite*
    /// direction: `--dir in` returning few/no `calls` edges is
    /// otherwise indistinguishable from "this symbol genuinely has few
    /// callers" vs. "N call sites spell this name in a shape this
    /// repo's extractor never attempted to resolve" (§1.1's honest
    /// absence signal, ADR-0020). Never itself evidence of a caller —
    /// see the field's own doc comment on `SymbolNode`.
    pub root_uncaptured_inbound_calls: u32,
    /// The root symbol's own [`crate::graph::SymbolNode::
    /// uncaptured_outbound_calls`] (0 for a File/Module root, or a
    /// language with no such exclusion) — the outbound counterpart to
    /// `root_uncaptured_inbound_calls` (ADR-0023): `--dir out` returning
    /// few/no `calls` edges is otherwise indistinguishable from "this
    /// symbol genuinely calls little" vs. "N of its own call sites spell
    /// a name in a shape this repo's extractor never attempted to
    /// resolve". Never itself evidence of what those calls target — see
    /// the field's own doc comment on `SymbolNode`.
    pub root_uncaptured_outbound_calls: u32,
    /// The root symbol's own [`crate::graph::SymbolNode::
    /// unresolved_inbound_calls`] (empty for a File/Module root) — the
    /// third honesty signal alongside `root_uncaptured_inbound_calls`
    /// (ADR-0033). Distinct from that field: this covers call sites that
    /// *were* attempted and produced no edge (most often bare-name
    /// ambiguity — an interface method and its implementation both
    /// named the same thing), not syntax an extractor never attempts at
    /// all. Not itself evidence any of these sites target the root.
    pub root_unresolved_inbound_calls: Vec<InboundCallSite>,
    /// Uncapped count backing `root_unresolved_inbound_calls` — see
    /// [`crate::graph::SymbolNode::unresolved_inbound_call_count`].
    pub root_unresolved_inbound_call_count: u32,
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

    let root_id = resolve_target(qg, &query.target, query.subpath.as_deref())?;
    let root = summarize(qg, &root_id);
    let root_file = owning_file(qg, &root_id);
    let root_unresolved_calls = match qg.node(&root_id).map(|n| &n.data) {
        Some(NodeData::Symbol(s)) => s.unresolved_calls.clone(),
        _ => Vec::new(),
    };
    let root_uncaptured_inbound_calls = match qg.node(&root_id).map(|n| &n.data) {
        Some(NodeData::Symbol(s)) => s.uncaptured_inbound_calls,
        _ => 0,
    };
    let root_uncaptured_outbound_calls = match qg.node(&root_id).map(|n| &n.data) {
        Some(NodeData::Symbol(s)) => s.uncaptured_outbound_calls,
        _ => 0,
    };
    let root_unresolved_inbound_calls = match qg.node(&root_id).map(|n| &n.data) {
        Some(NodeData::Symbol(s)) => s.unresolved_inbound_calls.clone(),
        _ => Vec::new(),
    };
    let root_unresolved_inbound_call_count = match qg.node(&root_id).map(|n| &n.data) {
        Some(NodeData::Symbol(s)) => s.unresolved_inbound_call_count,
        _ => 0,
    };

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
                // The traversal itself is never scoped — `other` is
                // always added to `next_frontier` so later hops can
                // still reach through an out-of-scope node (a chain
                // that dips outside `--subpath` and back must not be
                // silently broken). Only whether this edge gets
                // *reported* is scoped.
                if qg.path_in_scope(&other, query.subpath.as_deref()) {
                    let same_file_as_root = match (&root_file, owning_file(qg, &other)) {
                        (Some(rf), Some(of)) => *rf == of,
                        _ => false,
                    };
                    edges_this_hop.push(DepEdge {
                        kind: edge.kind,
                        confidence: edge.confidence,
                        evidence: edge.evidence.clone(),
                        direction,
                        node: summarize(qg, &other),
                        same_file_as_root,
                    });
                }
                next_frontier.push(other);
            }
        }

        // Stop once there's nothing left to explore — not once a hop
        // has nothing to *report*: with `--subpath` set, a hop can
        // legitimately discover only out-of-scope nodes while still
        // having real descendants worth reaching in a later hop.
        if next_frontier.is_empty() {
            break;
        }

        if !edges_this_hop.is_empty() {
            hops.push(Hop {
                depth,
                edges: edges_this_hop,
            });
        }
        frontier = next_frontier;

        if depth == query.depth && !frontier.is_empty() {
            hit_depth_cap = true;
        }
    }

    let truncation = match (hit_depth_cap, query.depth < consts::MAX_DEPS_DEPTH) {
        (true, true) => {
            // Carries --subpath forward too, if set — spec §7.2's "the
            // exact follow-up call to get more" should reproduce the
            // same scoped view, not silently drop the restriction.
            let subpath_flag = query
                .subpath
                .as_deref()
                .map(|s| format!(" --subpath {s}"))
                .unwrap_or_default();
            Truncation::more(format!(
                "carto deps {} --dir {} --depth {}{subpath_flag}",
                query.target,
                query.dir.as_str(),
                query.depth + 1
            ))
        }
        (true, false) => Truncation {
            truncated: true,
            next_call: None,
        },
        (false, _) => Truncation::none(),
    };

    Ok(DepsResult {
        root,
        root_unresolved_calls,
        root_uncaptured_inbound_calls,
        root_uncaptured_outbound_calls,
        root_unresolved_inbound_calls,
        root_unresolved_inbound_call_count,
        hops,
        truncation,
    })
}

/// One candidate a bare `target` string resolved to, before the
/// ambiguity check — carries enough to both extract the `NodeId` and
/// describe itself in a multi-match error message, since the three
/// kinds have no field in common besides the ID (ADR-0021).
enum Candidate {
    Symbol(NodeId, String),
    Module(NodeId, String),
    File(NodeId, String),
}

impl Candidate {
    fn id(&self) -> &NodeId {
        match self {
            Candidate::Symbol(id, _) | Candidate::Module(id, _) | Candidate::File(id, _) => id,
        }
    }

    fn describe(&self) -> String {
        match self {
            Candidate::Symbol(id, location) => format!("{id} ({location})"),
            Candidate::Module(id, path) => format!("{id} (module {path})"),
            Candidate::File(id, path) => format!("{id} (file {path})"),
        }
    }
}

/// Resolves `target` to a [`NodeId`]: used as-is if it's already an
/// existing node's ID; otherwise looked up as an exact match against a
/// symbol name, a `Module.path`, or a `File.path` (ADR-0021 widens this
/// from symbol-name-only — a deps root is at least as often a file or
/// package as a specific symbol, and node IDs are blake3 hashes with no
/// other reachable lookup path). Symbol/module matching reuses
/// [`super::find`]; file matching is a direct scan here, since `find`
/// deliberately doesn't search `File` nodes (see `find.rs`'s module
/// doc). `subpath` is deliberately **not** applied to this initial
/// lookup — the deps root is very commonly outside the subtree being
/// reported on (e.g. `deps handle --subpath src/orders` to see only
/// `handle`'s dependencies that land in `orders`, where `handle` itself
/// lives elsewhere entirely), so scoping the root lookup unconditionally
/// would break that common case. `subpath` only comes into play as a
/// **tiebreaker**: if the unscoped lookup is ambiguous (more than one
/// candidate), it's retried scoped, so `--subpath` can still resolve an
/// otherwise-ambiguous name (e.g. the same symbol name present in a live
/// tree and in some unrelated vendored/dead-code directory) — a side
/// effect of narrowing the candidate set, not a separate mechanism, and
/// never invoked for a name that was already unambiguous. Zero matches
/// or more than one match (after any tiebreak attempt) are both
/// `UserError`s (INV-8's honesty rule, applied to the CLI surface: no
/// silent pick among ambiguous candidates) — the multi-match case lists
/// every candidate's ID so the caller can re-run with one.
fn resolve_target(qg: &QueryGraph, target: &str, subpath: Option<&str>) -> Result<NodeId> {
    if let Some(node) = qg.nodes().find(|n| n.id.as_str() == target) {
        return Ok(node.id.clone());
    }

    let candidates = |scope: Option<&str>| -> Vec<Candidate> {
        let find_query = FindQuery {
            needle: target.to_string(),
            exact: true,
            limit: usize::MAX,
            subpath: scope.map(str::to_string),
        };
        let found = find(qg, &find_query);
        let mut out: Vec<Candidate> = found
            .matches
            .into_iter()
            .map(|m| Candidate::Symbol(m.id, m.location))
            .collect();
        out.extend(
            found
                .module_matches
                .into_iter()
                .map(|m| Candidate::Module(m.id, m.path)),
        );
        for node in qg.nodes() {
            if let NodeData::File(f) = &node.data {
                if f.path == target && qg.path_in_scope(&node.id, scope) {
                    out.push(Candidate::File(node.id.clone(), f.path.clone()));
                }
            }
        }
        out
    };

    let unscoped = candidates(None);
    let matches = if unscoped.len() > 1 && subpath.is_some() {
        candidates(subpath)
    } else {
        unscoped
    };

    match matches.len() {
        0 => Err(Error::new(
            ErrorKind::UserError,
            format!("no node ID or symbol named `{target}` found in this index"),
        )),
        1 => Ok(matches.into_iter().next().unwrap().id().clone()),
        _ => {
            let descriptions: Vec<String> = matches.iter().map(Candidate::describe).collect();
            Err(Error::new(
                ErrorKind::UserError,
                format!(
                    "`{target}` matches multiple nodes; specify one by ID: {}",
                    descriptions.join(", ")
                ),
            ))
        }
    }
}

/// The `File` node `id` "lives in", for `same_file_as_root` (§1.4):
/// a `Symbol`'s own `file` field; a `File` node is its own answer;
/// `None` for a `Module` (no file) or a dangling reference.
fn owning_file(qg: &QueryGraph, id: &NodeId) -> Option<NodeId> {
    match qg.node(id).map(|n| &n.data) {
        Some(NodeData::Symbol(s)) => Some(s.file.clone()),
        Some(NodeData::File(_)) => Some(id.clone()),
        Some(NodeData::Module(_)) | Some(NodeData::Contract(_)) | None => None,
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
        // `value` is tainted (ADR-0026) — `render_capped` is the INV-5
        // accessor, same as every other tainted-field-to-plain-`String`
        // conversion in this crate (e.g. a symbol's `signature`).
        Some(data @ NodeData::Contract(c)) => NodeSummary {
            id: id.clone(),
            kind: data.kind_str().to_string(),
            label: c.value.render_capped(crate::consts::SIGNATURE_CAP),
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
                component: None,
            },
        )
    }

    fn symbol(relpath: &str, name: &str, file_id: &NodeId, line: u32) -> Node {
        symbol_with_unresolved(relpath, name, file_id, line, vec![])
    }

    fn symbol_with_unresolved(
        relpath: &str,
        name: &str,
        file_id: &NodeId,
        line: u32,
        unresolved_calls: Vec<UnresolvedCall>,
    ) -> Node {
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
                unresolved_calls,
                uncaptured_inbound_calls: 0,
                uncaptured_outbound_calls: 0,
                unresolved_inbound_calls: vec![],
                unresolved_inbound_call_count: 0,
            },
        )
    }

    fn symbol_with_uncaptured_outbound(
        relpath: &str,
        name: &str,
        file_id: &NodeId,
        line: u32,
        uncaptured_outbound_calls: u32,
    ) -> Node {
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
                uncaptured_outbound_calls,
                unresolved_inbound_calls: vec![],
                unresolved_inbound_call_count: 0,
            },
        )
    }

    fn symbol_with_unresolved_inbound(
        relpath: &str,
        name: &str,
        file_id: &NodeId,
        line: u32,
        unresolved_inbound_calls: Vec<InboundCallSite>,
        unresolved_inbound_call_count: u32,
    ) -> Node {
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
                unresolved_inbound_calls,
                unresolved_inbound_call_count,
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

    /// `handle` (in `mg_site/`) -> `parse_order` (in `vendor/`, out of
    /// scope for `--subpath mg_site`) -> `validate` (back in `mg_site/`)
    /// — a chain that dips *out* of the subtree and back, for proving
    /// the BFS traversal itself stays unrestricted by `--subpath` even
    /// though the middle hop's edge is hidden from the report.
    fn cross_boundary_chain_doc() -> GraphDocument {
        let mg_site = file("mg_site/handlers.rs");
        let vendor = file("vendor/orders.rs");
        let mg_site_orders = file("mg_site/orders.rs");
        let handle = symbol("mg_site/handlers.rs", "handle", &mg_site.id, 1);
        let parse_order = symbol("vendor/orders.rs", "parse_order", &vendor.id, 1);
        let validate = symbol("mg_site/orders.rs", "validate", &mg_site_orders.id, 1);
        let e1 = Edge::new(
            EdgeKind::Calls,
            handle.id.clone(),
            parse_order.id.clone(),
            Confidence::Inferred,
            "same-package".to_string(),
        );
        let e2 = Edge::new(
            EdgeKind::Calls,
            parse_order.id.clone(),
            validate.id.clone(),
            Confidence::Inferred,
            "same-package".to_string(),
        );
        doc(
            vec![
                mg_site,
                vendor,
                mg_site_orders,
                handle,
                parse_order,
                validate,
            ],
            vec![e1, e2],
        )
    }

    #[test]
    fn subpath_hides_an_out_of_scope_hop_but_the_bfs_still_reaches_past_it() {
        let qg = QueryGraph::from_document(cross_boundary_chain_doc());
        let query = DepsQuery {
            target: "handle".to_string(),
            dir: Direction::Out,
            depth: 2,
            kinds: None,
            subpath: Some("mg_site".to_string()),
        };
        let result = run(&qg, &query).unwrap();

        // Only one hop is reported (depth 2's edge to `validate`) — hop
        // 1's edge (to the out-of-scope `parse_order`) is hidden, but
        // the traversal still advanced through it to reach `validate`.
        assert_eq!(result.hops.len(), 1);
        assert_eq!(result.hops[0].depth, 2);
        assert_eq!(result.hops[0].edges[0].node.label, "validate");
    }

    #[test]
    fn subpath_none_reports_every_hop_as_before() {
        let qg = QueryGraph::from_document(cross_boundary_chain_doc());
        let result = run(
            &qg,
            &DepsQuery {
                target: "handle".to_string(),
                dir: Direction::Out,
                depth: 2,
                kinds: None,
                subpath: None,
            },
        )
        .unwrap();
        assert_eq!(result.hops.len(), 2);
        assert_eq!(result.hops[0].edges[0].node.label, "parse_order");
        assert_eq!(result.hops[1].edges[0].node.label, "validate");
    }

    #[test]
    fn subpath_narrows_ambiguous_target_resolution() {
        // Same symbol name in two directories — `--subpath` picks the
        // one under it instead of erroring as ambiguous, directly
        // serving the "same name in the live tree and in an unrelated
        // vendored directory" disambiguation case.
        let live = file("mg_site/orders.rs");
        let dead = file("typo3_v8_delete_me/orders.rs");
        let live_sym = symbol("mg_site/orders.rs", "Div", &live.id, 1);
        let dead_sym = symbol("typo3_v8_delete_me/orders.rs", "Div", &dead.id, 1);
        let qg =
            QueryGraph::from_document(doc(vec![live, dead, live_sym.clone(), dead_sym], vec![]));

        let query = DepsQuery {
            target: "Div".to_string(),
            dir: Direction::Out,
            depth: 1,
            kinds: None,
            subpath: Some("mg_site".to_string()),
        };
        let result = run(&qg, &query).unwrap();
        assert_eq!(result.root.id, live_sym.id);
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
            subpath: None,
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
                subpath: None,
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
            subpath: None,
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
            subpath: None,
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
            subpath: None,
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

    /// ADR-0021: `deps`'s target resolution used to error outright on a
    /// `Module`/`File` path (spec §7.1's L3 finding) since only symbol
    /// names were matched — node IDs are blake3 hashes with no other
    /// reachable lookup path, so a package or file was unreachable from
    /// the tool surface entirely.
    #[test]
    fn target_can_be_a_module_path() {
        let f = file("src/lib.rs");
        let module_id = crate::graph::module_id("serde", true);
        let module = Node::module(
            module_id.clone(),
            Provenance::Syntactic,
            "test@1",
            crate::graph::ModuleNode {
                path: "serde".to_string(),
                external: true,
            },
        );
        let e = Edge::new(
            EdgeKind::Imports,
            f.id.clone(),
            module_id.clone(),
            Confidence::Certain,
            "external-package".to_string(),
        );
        let qg = QueryGraph::from_document(doc(vec![f, module], vec![e]));

        let result = run(
            &qg,
            &DepsQuery {
                target: "serde".to_string(),
                dir: Direction::In,
                depth: 1,
                kinds: None,
                subpath: None,
            },
        )
        .unwrap();
        assert_eq!(result.root.id, module_id);
        assert_eq!(result.root.kind, "module");
        assert_eq!(result.hops[0].edges[0].node.label, "src/lib.rs");
    }

    #[test]
    fn target_can_be_a_file_path() {
        let qg = QueryGraph::from_document(chain_doc());
        let result = run(&qg, &DepsQuery::new("src/lib.rs")).unwrap();
        assert_eq!(result.root.kind, "file");
        assert_eq!(result.root.label, "src/lib.rs");
    }

    #[test]
    fn confidence_and_evidence_are_surfaced_on_every_row() {
        let qg = QueryGraph::from_document(chain_doc());
        let result = run(&qg, &DepsQuery::new("handle")).unwrap();
        assert_eq!(result.hops[0].edges[0].confidence, Confidence::Inferred);
        assert_eq!(result.hops[0].edges[0].evidence, vec!["same-file"]);
    }

    /// T5: a caller living in the same file as the symbol it calls was
    /// misread as "the method definition itself" rather than a distinct
    /// call site — `chain_doc`'s `handle`/`parse_order` both live in
    /// `src/lib.rs`, matching that failure mode exactly.
    #[test]
    fn same_file_as_root_is_true_when_caller_and_callee_share_a_file() {
        let qg = QueryGraph::from_document(chain_doc());
        let result = run(&qg, &DepsQuery::new("handle")).unwrap();
        assert!(result.hops[0].edges[0].same_file_as_root);
    }

    #[test]
    fn same_file_as_root_is_false_across_files() {
        // handle (mg_site/handlers.rs) -> parse_order (vendor/orders.rs)
        // — different files, unlike chain_doc's same-file case above.
        let qg = QueryGraph::from_document(cross_boundary_chain_doc());
        let result = run(
            &qg,
            &DepsQuery {
                target: "handle".to_string(),
                dir: Direction::Out,
                depth: 1,
                kinds: None,
                subpath: None,
            },
        )
        .unwrap();
        assert!(!result.hops[0].edges[0].same_file_as_root);
    }

    #[test]
    fn same_file_as_root_is_false_for_a_module_endpoint() {
        let f = file("src/lib.rs");
        let module_id = crate::graph::module_id("serde", true);
        let module = Node::module(
            module_id.clone(),
            Provenance::Syntactic,
            "test@1",
            crate::graph::ModuleNode {
                path: "serde".to_string(),
                external: true,
            },
        );
        let e = Edge::new(
            EdgeKind::Imports,
            f.id.clone(),
            module_id,
            Confidence::Certain,
            "external-package".to_string(),
        );
        let qg = QueryGraph::from_document(doc(vec![f, module], vec![e]));
        let result = run(&qg, &DepsQuery::new("src/lib.rs")).unwrap();
        assert!(!result.hops[0].edges[0].same_file_as_root);
    }

    #[test]
    fn root_unresolved_calls_are_surfaced_when_present() {
        // A large static-utility-class scenario: the root symbol calls
        // plenty, but every call lands in unresolved_calls (ambiguous
        // candidates, or calls into code this repo never indexed) —
        // `--dir out` returning zero edges must not be indistinguishable
        // from "genuinely calls nothing."
        let f = file("src/lib.rs");
        let div = symbol_with_unresolved(
            "src/lib.rs",
            "Div",
            &f.id,
            1,
            vec![
                UnresolvedCall {
                    name: "trimExplode".to_string(),
                    line: 5,
                },
                UnresolvedCall {
                    name: "isValidUrl".to_string(),
                    line: 9,
                },
            ],
        );
        let qg = QueryGraph::from_document(doc(vec![f, div], vec![]));

        let result = run(&qg, &DepsQuery::new("Div")).unwrap();
        assert_eq!(result.hops.len(), 0, "no resolved edges");
        assert_eq!(
            result.root_unresolved_calls,
            vec![
                UnresolvedCall {
                    name: "trimExplode".to_string(),
                    line: 5
                },
                UnresolvedCall {
                    name: "isValidUrl".to_string(),
                    line: 9
                },
            ]
        );
    }

    #[test]
    fn root_unresolved_calls_is_empty_when_the_root_has_none() {
        let qg = QueryGraph::from_document(chain_doc());
        let result = run(&qg, &DepsQuery::new("handle")).unwrap();
        assert!(result.root_unresolved_calls.is_empty());
    }

    #[test]
    fn root_unresolved_calls_is_empty_for_a_non_symbol_root() {
        let f = file("src/lib.rs");
        let qg = QueryGraph::from_document(doc(vec![f.clone()], vec![]));
        let result = run(&qg, &DepsQuery::new(f.id.as_str())).unwrap();
        assert!(result.root_unresolved_calls.is_empty());
    }

    #[test]
    fn root_unresolved_calls_is_unaffected_by_dir_and_kinds_filtering() {
        // Root-node metadata, not traversal output — --dir/--kinds only
        // ever filter `hops`.
        let f = file("src/lib.rs");
        let div = symbol_with_unresolved(
            "src/lib.rs",
            "Div",
            &f.id,
            1,
            vec![UnresolvedCall {
                name: "trimExplode".to_string(),
                line: 5,
            }],
        );
        let qg = QueryGraph::from_document(doc(vec![f, div], vec![]));

        let mut kinds = BTreeSet::new();
        kinds.insert(EdgeKind::Contains);
        let query = DepsQuery {
            target: "Div".to_string(),
            dir: Direction::In,
            depth: 1,
            kinds: Some(kinds),
            subpath: None,
        };
        let result = run(&qg, &query).unwrap();
        assert_eq!(result.root_unresolved_calls.len(), 1);
    }

    // ADR-0023: root_uncaptured_outbound_calls, mirroring the
    // root_unresolved_calls tests above exactly — same root-level-
    // metadata shape, opposite direction.

    #[test]
    fn root_uncaptured_outbound_calls_is_surfaced_when_present() {
        let f = file("src/indexer.rs");
        let build_and_persist =
            symbol_with_uncaptured_outbound("src/indexer.rs", "build_and_persist", &f.id, 1, 6);
        let qg = QueryGraph::from_document(doc(vec![f, build_and_persist], vec![]));

        let result = run(&qg, &DepsQuery::new("build_and_persist")).unwrap();
        assert_eq!(result.hops.len(), 0, "no resolved edges");
        assert_eq!(result.root_uncaptured_outbound_calls, 6);
    }

    #[test]
    fn root_uncaptured_outbound_calls_is_zero_when_the_root_has_none() {
        let qg = QueryGraph::from_document(chain_doc());
        let result = run(&qg, &DepsQuery::new("handle")).unwrap();
        assert_eq!(result.root_uncaptured_outbound_calls, 0);
    }

    #[test]
    fn root_uncaptured_outbound_calls_is_zero_for_a_non_symbol_root() {
        let f = file("src/lib.rs");
        let qg = QueryGraph::from_document(doc(vec![f.clone()], vec![]));
        let result = run(&qg, &DepsQuery::new(f.id.as_str())).unwrap();
        assert_eq!(result.root_uncaptured_outbound_calls, 0);
    }

    #[test]
    fn root_uncaptured_outbound_calls_is_unaffected_by_dir_and_kinds_filtering() {
        // Root-node metadata, not traversal output — --dir/--kinds only
        // ever filter `hops`.
        let f = file("src/indexer.rs");
        let build_and_persist =
            symbol_with_uncaptured_outbound("src/indexer.rs", "build_and_persist", &f.id, 1, 6);
        let qg = QueryGraph::from_document(doc(vec![f, build_and_persist], vec![]));

        let mut kinds = BTreeSet::new();
        kinds.insert(EdgeKind::Contains);
        let query = DepsQuery {
            target: "build_and_persist".to_string(),
            dir: Direction::In,
            depth: 1,
            kinds: Some(kinds),
            subpath: None,
        };
        let result = run(&qg, &query).unwrap();
        assert_eq!(result.root_uncaptured_outbound_calls, 6);
    }

    // ADR-0033: root_unresolved_inbound_calls — the third honesty
    // signal, mirroring the two families above's shape but answering a
    // question neither covers: call sites elsewhere that spell the
    // root's bare name, *were* attempted, and still produced no edge
    // (typically bare-name ambiguity, e.g. an interface method and its
    // implementation).

    #[test]
    fn root_unresolved_inbound_calls_are_surfaced_when_present() {
        let f = file("src/lib.rs");
        let save = symbol_with_unresolved_inbound(
            "src/lib.rs",
            "Save",
            &f.id,
            1,
            vec![
                InboundCallSite {
                    file: "src/CancelQueryUseCase.cs".to_string(),
                    line: 77,
                },
                InboundCallSite {
                    file: "src/GetQueryStatusUseCase.cs".to_string(),
                    line: 102,
                },
            ],
            2,
        );
        let qg = QueryGraph::from_document(doc(vec![f, save], vec![]));

        let result = run(&qg, &DepsQuery::new("Save")).unwrap();
        assert_eq!(result.hops.len(), 0, "no resolved edges");
        assert_eq!(result.root_unresolved_inbound_call_count, 2);
        assert_eq!(
            result.root_unresolved_inbound_calls,
            vec![
                InboundCallSite {
                    file: "src/CancelQueryUseCase.cs".to_string(),
                    line: 77,
                },
                InboundCallSite {
                    file: "src/GetQueryStatusUseCase.cs".to_string(),
                    line: 102,
                },
            ]
        );
    }

    #[test]
    fn root_unresolved_inbound_calls_is_empty_when_the_root_has_none() {
        let qg = QueryGraph::from_document(chain_doc());
        let result = run(&qg, &DepsQuery::new("handle")).unwrap();
        assert!(result.root_unresolved_inbound_calls.is_empty());
        assert_eq!(result.root_unresolved_inbound_call_count, 0);
    }

    #[test]
    fn root_unresolved_inbound_calls_is_empty_for_a_non_symbol_root() {
        let f = file("src/lib.rs");
        let qg = QueryGraph::from_document(doc(vec![f.clone()], vec![]));
        let result = run(&qg, &DepsQuery::new(f.id.as_str())).unwrap();
        assert!(result.root_unresolved_inbound_calls.is_empty());
        assert_eq!(result.root_unresolved_inbound_call_count, 0);
    }

    #[test]
    fn truncation_suggests_a_deeper_depth_when_the_cap_stopped_traversal() {
        let qg = QueryGraph::from_document(chain_doc());
        let query = DepsQuery {
            target: "handle".to_string(),
            dir: Direction::Out,
            depth: 1,
            kinds: None,
            subpath: None,
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
            subpath: None,
        };
        let result = run(&qg, &query).unwrap();
        assert!(!result.truncation.truncated);
    }
}
