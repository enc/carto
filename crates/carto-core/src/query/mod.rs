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

pub mod contract;
pub mod deps;
pub mod find;
pub mod map;
pub mod orphans;

use crate::components::Component;
use crate::error::{Error, Result};
use crate::graph::{Edge, EdgeId, GraphDocument, Node, NodeData, NodeId, SymbolNode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub use contract::{ContractMatch, ContractQuery, ContractResult, ContractSite, run as contract};
pub use deps::{DepEdge, DepsQuery, DepsResult, Hop, NodeSummary, run as deps};
pub use find::{FindQuery, FindResult, ModuleMatch, SymbolMatch, run as find};
pub use map::{MapCounts, MapQuery, MapResult, MapSection, run as map};
pub use orphans::{OrphanContract, OrphansQuery, OrphansResult, run as orphans};

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
    /// `GraphDocument::components` verbatim, sorted by path (INV-7) as
    /// persisted — ADR-0034/0035. Small (repo-scale, not node-scale), so
    /// a plain `Vec` scanned linearly by `known_component_names` is
    /// fine; per-node lookups go through `File`/`Symbol` nodes' own
    /// `component` field instead (`component_of`/`component_in_scope`
    /// below), not this table.
    components: Vec<Component>,
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
            components: doc.components,
        }
    }

    /// Every component this index knows about (ADR-0034/0035), in the
    /// order persisted (path-sorted). Used to render `map`'s components
    /// section and to validate a `--component` flag's value, listing the
    /// known names in a `UserError` when it doesn't match any of them.
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// The set of every known component's `name`, for `--component`
    /// validation — a name that isn't in this set is a typo, not a
    /// legitimately-empty-result query.
    pub fn known_component_names(&self) -> BTreeSet<&str> {
        self.components.iter().map(|c| c.name.as_str()).collect()
    }

    /// Renders a `--component` filter as a `" --component X --component
    /// Y"` suffix for a `Truncation::next_call` hint — shared by every
    /// command that carries this filter forward, so the formatting rule
    /// (an absent or empty filter contributes nothing) lives in one
    /// place rather than five copies of the same `.filter(...).map(...)
    /// .unwrap_or_default()`.
    pub fn component_flags(component: Option<&BTreeSet<String>>) -> String {
        component
            .filter(|c| !c.is_empty())
            .map(|c| {
                c.iter()
                    .map(|name| format!(" --component {name}"))
                    .collect::<String>()
            })
            .unwrap_or_default()
    }

    /// Validates a `--component` filter's names against what this index
    /// actually knows about (`known_component_names`) — an unknown name
    /// is a typo, not a legitimately-empty-result query, unlike
    /// `--subpath`, which has no enumerable "known directories" list to
    /// check a value against. Called once at the front-end layer (each
    /// CLI command/MCP tool, right after building `qg` and before
    /// constructing its query struct) rather than inside the core query
    /// functions themselves, which stay infallible — the same reason
    /// `deps::run`'s own `--depth` cap check lives in that fallible
    /// wrapper rather than being duplicated everywhere `depth` is used.
    pub fn validate_component_filter(&self, filter: Option<&BTreeSet<String>>) -> Result<()> {
        let Some(filter) = filter else {
            return Ok(());
        };
        let known = self.known_component_names();
        let mut unknown: Vec<&String> = filter
            .iter()
            .filter(|n| !known.contains(n.as_str()))
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        unknown.sort();
        let unknown_list = unknown
            .iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let known_list = if known.is_empty() {
            "(none in this index)".to_string()
        } else {
            known.into_iter().collect::<Vec<_>>().join(", ")
        };
        Err(Error::user(format!(
            "unknown --component value(s): {unknown_list} — known components: {known_list}"
        )))
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
            // Same "no directory of its own" reasoning as `Module`
            // (ADR-0014) — a Contract is scoped by its `produces`/
            // `consumes` edges' endpoints, not a path of its own.
            Some(NodeData::Module(_)) | Some(NodeData::Contract(_)) => return true,
            None => return false,
        };
        path == prefix || path.starts_with(&format!("{prefix}/"))
    }

    /// The component (ADR-0034/0035) `id`'s node belongs to, for
    /// *display* — a `Symbol`'s component is its owning file's; a
    /// `File`'s is its own; a `Module`/`Contract`/dangling reference has
    /// none, `None`, the honest answer (neither has a directory of its
    /// own, mirroring `path_in_scope`'s treatment of the same two
    /// kinds — but see [`Self::component_in_scope`] below for why
    /// *scope-filtering* those two kinds is a different question with a
    /// different answer than this accessor gives).
    pub fn component_of(&self, id: &NodeId) -> Option<&str> {
        match self.node(id).map(|n| &n.data) {
            Some(NodeData::File(f)) => f.component.as_deref(),
            Some(NodeData::Symbol(s)) => self
                .node(&s.file)
                .and_then(|n| n.data.as_file())
                .and_then(|f| f.component.as_deref()),
            _ => None,
        }
    }

    /// Whether `id` is in scope for a `--component <NAME>` filter
    /// (repeatable — `filter` is the resulting name set), the exact
    /// component analogue of [`Self::path_in_scope`] for `--subpath`,
    /// including its governing principle (ADR-0014): restricts which
    /// rows get *listed*, never what a traversal/aggregation actually
    /// computes. `filter: None` or an empty set means "no restriction" —
    /// always `true`, the same tolerance `path_in_scope` gives an
    /// empty/whitespace-only `--subpath`.
    ///
    /// `Module`/`Contract` nodes are **always** in scope here, unlike
    /// what [`Self::component_of`] would report for them (`None`) — the
    /// same "no directory/component of its own, so never excluded by
    /// this kind of filter" rule `path_in_scope` already applies to
    /// them, deliberately kept even though it means this predicate and
    /// `component_of` disagree for these two kinds on purpose: one
    /// answers "what component is this," the other "should this be
    /// hidden by a component filter," and for `Module`/`Contract` those
    /// are different questions with different honest answers.
    pub fn component_in_scope(&self, id: &NodeId, filter: Option<&BTreeSet<String>>) -> bool {
        let Some(filter) = filter else { return true };
        if filter.is_empty() {
            return true;
        }
        // Module/Contract are always in scope (see this method's own
        // doc comment for why that's a deliberate divergence from what
        // `component_of` reports for them); everything else defers to
        // `component_of`'s own File/Symbol-via-owning-file lookup
        // rather than repeating it here.
        match self.node(id).map(|n| &n.data) {
            Some(NodeData::Module(_)) | Some(NodeData::Contract(_)) => true,
            Some(NodeData::File(_)) | Some(NodeData::Symbol(_)) => self
                .component_of(id)
                .is_some_and(|c| self.component_matches_filter(c, filter)),
            None => false,
        }
    }

    /// ADR-0038: whether component `name` is one of `filter`'s own
    /// entries, or nested *under* one of them — `--component <name>`
    /// scopes to `name` and everything nested under it, not just
    /// components whose own name equals it exactly. Nesting is real
    /// today even without a dedicated "parent" field: a component
    /// discovered from a manifest marker inside another component's own
    /// directory (`services/api/go.mod` and `services/api/internal/
    /// go.mod`, `components::tests::
    /// innermost_component_wins_for_nested_markers`) is a second,
    /// separate `Component` whose `path` happens to be a subdirectory
    /// of the first's — exactly the shape this method detects.
    ///
    /// Falls back to `filter.contains(name)` alone for a `name` this
    /// index has no `Component` table entry for (should not happen for
    /// a filter value that passed `validate_component_filter`, or for a
    /// `name` read from a real `FileNode.component`, but stays
    /// conservative rather than panicking on a hand-edited `graph.json`
    /// where the two could disagree).
    pub fn component_matches_filter(&self, name: &str, filter: &BTreeSet<String>) -> bool {
        if filter.contains(name) {
            return true;
        }
        let Some(path) = self.component_path(name) else {
            return false;
        };
        filter.iter().any(|f| {
            self.component_path(f).is_some_and(|ancestor_path| {
                path.strip_prefix(ancestor_path)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
        })
    }

    fn component_path(&self, name: &str) -> Option<&str> {
        self.components
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.path.as_str())
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
                component: None,
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
            uncaptured_outbound_calls: 0,
            unresolved_inbound_calls: vec![],
            unresolved_inbound_call_count: 0,
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
            uncaptured_outbound_calls: 0,
            unresolved_inbound_calls: vec![],
            unresolved_inbound_call_count: 0,
        });
        assert_eq!(rendered, "<unknown-file>:1-1");
    }

    fn file_node_with_component(path: &str, component: Option<&str>) -> Node {
        let mut n = file_node(path);
        if let crate::graph::NodeData::File(f) = &mut n.data {
            f.component = component.map(str::to_string);
        }
        n
    }

    fn doc_with_components(
        nodes: Vec<Node>,
        edges: Vec<Edge>,
        components: Vec<crate::components::Component>,
    ) -> GraphDocument {
        GraphDocument {
            components,
            ..doc(nodes, edges)
        }
    }

    #[test]
    fn component_of_resolves_a_symbol_through_its_owning_file() {
        let file = crate::graph::file_id("services/orders/handler.rs");
        let sym = symbol_node("services/orders/handler.rs", "run", &file);
        let sym_id = sym.id.clone();
        let qg = QueryGraph::from_document(doc(
            vec![
                file_node_with_component("services/orders/handler.rs", Some("orders")),
                sym,
            ],
            vec![],
        ));
        assert_eq!(qg.component_of(&file), Some("orders"));
        assert_eq!(qg.component_of(&sym_id), Some("orders"));
    }

    #[test]
    fn component_of_is_none_for_module_and_dangling_ids() {
        let module = Node::module(
            crate::graph::module_id("serde", true),
            Provenance::Syntactic,
            "test@1",
            crate::graph::ModuleNode {
                path: "serde".to_string(),
                external: true,
            },
        );
        let module_id = module.id.clone();
        let qg = QueryGraph::from_document(doc(vec![module], vec![]));
        assert_eq!(qg.component_of(&module_id), None);
        assert_eq!(qg.component_of(&crate::graph::file_id("nowhere.rs")), None);
    }

    #[test]
    fn component_in_scope_none_or_empty_filter_means_no_restriction() {
        let qg = QueryGraph::from_document(doc(
            vec![file_node_with_component(
                "services/orders/a.rs",
                Some("orders"),
            )],
            vec![],
        ));
        let id = crate::graph::file_id("services/orders/a.rs");
        assert!(qg.component_in_scope(&id, None));
        assert!(qg.component_in_scope(&id, Some(&BTreeSet::new())));
    }

    #[test]
    fn component_in_scope_filters_files_and_symbols_by_owning_file() {
        let orders_file_id = crate::graph::file_id("services/orders/a.rs");
        let billing_file_id = crate::graph::file_id("services/billing/b.rs");
        let sym = symbol_node("services/orders/a.rs", "run", &orders_file_id);
        let sym_id = sym.id.clone();
        let qg = QueryGraph::from_document(doc(
            vec![
                file_node_with_component("services/orders/a.rs", Some("orders")),
                file_node_with_component("services/billing/b.rs", Some("billing")),
                sym,
            ],
            vec![],
        ));
        let filter: BTreeSet<String> = ["orders".to_string()].into_iter().collect();
        assert!(qg.component_in_scope(&orders_file_id, Some(&filter)));
        assert!(qg.component_in_scope(&sym_id, Some(&filter)));
        assert!(!qg.component_in_scope(&billing_file_id, Some(&filter)));
    }

    #[test]
    fn component_in_scope_excludes_a_file_with_no_component_under_a_real_filter() {
        let qg = QueryGraph::from_document(doc(vec![file_node("scripts/a.rs")], vec![]));
        let id = crate::graph::file_id("scripts/a.rs");
        let filter: BTreeSet<String> = ["orders".to_string()].into_iter().collect();
        assert!(!qg.component_in_scope(&id, Some(&filter)));
    }

    #[test]
    fn component_in_scope_always_true_for_module_and_contract_nodes() {
        let module = Node::module(
            crate::graph::module_id("serde", true),
            Provenance::Syntactic,
            "test@1",
            crate::graph::ModuleNode {
                path: "serde".to_string(),
                external: true,
            },
        );
        let module_id = module.id.clone();
        let qg = QueryGraph::from_document(doc(vec![module], vec![]));
        let filter: BTreeSet<String> = ["orders".to_string()].into_iter().collect();
        assert!(qg.component_in_scope(&module_id, Some(&filter)));
    }

    #[test]
    fn component_in_scope_includes_a_component_nested_under_the_filtered_one() {
        // ADR-0038: `services/api/internal` is its own component
        // (innermost-marker-wins, `components::tests::
        // innermost_component_wins_for_nested_markers`), nested inside
        // `services/api`'s own directory. `--component api` must still
        // include a file whose own component is the nested
        // `internal` one, not just files labeled `api` exactly.
        let components = vec![
            crate::components::Component {
                name: "api".to_string(),
                path: "services/api".to_string(),
                kind: "go".to_string(),
                depends_on: vec![],
            },
            crate::components::Component {
                name: "internal".to_string(),
                path: "services/api/internal".to_string(),
                kind: "go".to_string(),
                depends_on: vec![],
            },
        ];
        let qg = QueryGraph::from_document(doc_with_components(
            vec![
                file_node_with_component("services/api/main.go", Some("api")),
                file_node_with_component("services/api/internal/x.go", Some("internal")),
            ],
            vec![],
            components,
        ));
        let filter: BTreeSet<String> = ["api".to_string()].into_iter().collect();
        assert!(qg.component_in_scope(
            &crate::graph::file_id("services/api/main.go"),
            Some(&filter)
        ));
        assert!(qg.component_in_scope(
            &crate::graph::file_id("services/api/internal/x.go"),
            Some(&filter)
        ));
    }

    #[test]
    fn component_in_scope_does_not_include_the_ancestor_when_filtering_on_the_nested_one() {
        // The reverse direction is *not* symmetric: `--component
        // internal` must not pull in `api`'s own top-level files —
        // descendant scoping only ever widens outward from the
        // requested name, never inward.
        let components = vec![
            crate::components::Component {
                name: "api".to_string(),
                path: "services/api".to_string(),
                kind: "go".to_string(),
                depends_on: vec![],
            },
            crate::components::Component {
                name: "internal".to_string(),
                path: "services/api/internal".to_string(),
                kind: "go".to_string(),
                depends_on: vec![],
            },
        ];
        let qg = QueryGraph::from_document(doc_with_components(
            vec![
                file_node_with_component("services/api/main.go", Some("api")),
                file_node_with_component("services/api/internal/x.go", Some("internal")),
            ],
            vec![],
            components,
        ));
        let filter: BTreeSet<String> = ["internal".to_string()].into_iter().collect();
        assert!(!qg.component_in_scope(
            &crate::graph::file_id("services/api/main.go"),
            Some(&filter)
        ));
        assert!(qg.component_in_scope(
            &crate::graph::file_id("services/api/internal/x.go"),
            Some(&filter)
        ));
    }

    #[test]
    fn component_matches_filter_falls_back_to_exact_match_with_no_component_table_entry() {
        // A name with no corresponding `Component` table row (shouldn't
        // happen for anything that passed `validate_component_filter`,
        // but the method stays conservative rather than panicking) only
        // ever matches itself, never expands.
        let qg = QueryGraph::from_document(doc(vec![], vec![]));
        let filter: BTreeSet<String> = ["orders".to_string()].into_iter().collect();
        assert!(qg.component_matches_filter("orders", &filter));
        assert!(!qg.component_matches_filter("billing", &filter));
    }

    #[test]
    fn validate_component_filter_none_or_empty_always_passes() {
        let qg = QueryGraph::from_document(doc(vec![], vec![]));
        assert!(qg.validate_component_filter(None).is_ok());
        assert!(qg.validate_component_filter(Some(&BTreeSet::new())).is_ok());
    }

    #[test]
    fn validate_component_filter_rejects_an_unknown_name() {
        let components = vec![crate::components::Component {
            name: "orders".to_string(),
            path: "services/orders".to_string(),
            kind: "go".to_string(),
            depends_on: vec![],
        }];
        let qg = QueryGraph::from_document(doc_with_components(vec![], vec![], components));
        let filter: BTreeSet<String> = ["orders".to_string(), "typo".to_string()]
            .into_iter()
            .collect();
        let err = qg.validate_component_filter(Some(&filter)).unwrap_err();
        assert!(err.to_string().contains("typo"));
        assert!(err.to_string().contains("orders"));
    }

    #[test]
    fn validate_component_filter_accepts_every_known_name() {
        let components = vec![crate::components::Component {
            name: "orders".to_string(),
            path: "services/orders".to_string(),
            kind: "go".to_string(),
            depends_on: vec![],
        }];
        let qg = QueryGraph::from_document(doc_with_components(vec![], vec![], components));
        let filter: BTreeSet<String> = ["orders".to_string()].into_iter().collect();
        assert!(qg.validate_component_filter(Some(&filter)).is_ok());
    }

    #[test]
    fn known_component_names_and_components_reflect_the_document() {
        let components = vec![
            crate::components::Component {
                name: "billing".to_string(),
                path: "services/billing".to_string(),
                kind: "go".to_string(),
                depends_on: vec![],
            },
            crate::components::Component {
                name: "orders".to_string(),
                path: "services/orders".to_string(),
                kind: "go".to_string(),
                depends_on: vec![],
            },
        ];
        let qg = QueryGraph::from_document(doc_with_components(vec![], vec![], components));
        assert_eq!(qg.components().len(), 2);
        let names = qg.known_component_names();
        assert!(names.contains("billing"));
        assert!(names.contains("orders"));
        assert!(!names.contains("nonexistent"));
    }
}
