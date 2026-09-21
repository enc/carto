//! The serialisation choke-point (spec §6.5): exactly one function
//! ([`persist`]) writes `graph.json` and its accompanying
//! `manifest.json`. In order: redact (INV-6, stub this slice), validate
//! every edge endpoint exists, enforce the size cap (§4.4), write
//! atomically through [`crate::pathguard::PathGuard`]. Sorting is not a
//! separate step here — [`Graph::into_sorted_parts`] already returns
//! nodes/edges in ID order.

use super::{Edge, Graph, Node, NodeId};
use crate::consts;
use crate::error::{Error, ErrorKind, Result};
use crate::pathguard::PathGuard;
use crate::redact::{self, RedactionCounts};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The on-disk `graph.json` shape (spec §4.4). No timestamps anywhere in
/// this struct — INV-7 requires it to be byte-identical for the same
/// input tree; timestamps live only in [`Manifest`].
#[derive(Debug, Serialize, Deserialize)]
pub struct GraphDocument {
    pub carto_version: String,
    pub schema_version: u32,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// `manifest.json` (spec §4.4): mutable metadata living alongside
/// `graph.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Unix seconds. This slice has no incremental re-index (that's M5,
    /// which would read the previous manifest first to preserve
    /// `created_at`), so both fields are always equal here.
    pub created_at: u64,
    pub updated_at: u64,
    /// `None` when `repo_root` isn't inside a git working tree.
    pub commit_sha: Option<String>,
    /// Always `None` this slice: an honest dirty flag needs a real
    /// index/worktree comparison that a hand-rolled `.git/HEAD` reader
    /// doesn't give us (see
    /// docs/adr/0006-manifest-git-provenance.md). Left `Option` rather
    /// than defaulting to `false`, which would claim a clean tree we
    /// never actually checked.
    pub dirty: Option<bool>,
    pub file_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub redaction: RedactionCounts,
    pub ignore_rule_digest: String,
    /// repo-relative path -> sha256 hex, for M5's incremental re-index.
    /// Only covers files whose contents were actually read (excludes
    /// `excluded: sensitive` and `skipped: binary`/`too_large` files).
    pub file_sha256: BTreeMap<String, String>,
}

/// Everything [`persist`] needs beyond the graph itself — values only the
/// caller (`walk`, this slice) can produce.
pub struct PersistMeta {
    pub commit_sha: Option<String>,
    pub ignore_rule_digest: String,
    pub file_sha256: BTreeMap<String, String>,
}

/// The spec §6.5 choke-point. Consumes `graph` (persisting is the last
/// thing that happens to it) and writes `graph.json` + `manifest.json`
/// through `guard`, in order:
///
/// 1. redact (INV-6, no-op stub this slice);
/// 2. validate every edge endpoint resolves to a node in this graph
///    (`DataError` otherwise);
/// 3. enforce `consts::MAX_NODES` / `consts::MAX_GRAPH_BYTES` (§4.4);
/// 4. write atomically via [`PathGuard::writer`].
pub fn persist(mut graph: Graph, meta: PersistMeta, guard: &PathGuard) -> Result<Manifest> {
    let redaction = redact::redact(&mut graph);
    let (nodes, edges) = graph.into_sorted_parts();

    if nodes.len() > consts::MAX_NODES {
        return Err(Error::new(
            ErrorKind::DataError,
            format!(
                "graph has {} nodes, exceeding the {}-node cap; narrow the index with .cartoignore",
                nodes.len(),
                consts::MAX_NODES
            ),
        ));
    }

    validate_edge_endpoints(&nodes, &edges)?;

    let node_count = nodes.len();
    let edge_count = edges.len();
    let file_count = nodes.iter().filter(|n| n.data.as_file().is_some()).count();

    let doc = GraphDocument {
        carto_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: consts::SCHEMA_VERSION,
        nodes,
        edges,
    };

    let graph_bytes = serde_json::to_vec_pretty(&doc).map_err(|e| {
        Error::with_source(ErrorKind::DataError, "failed to serialize graph.json", e)
    })?;

    if graph_bytes.len() as u64 > consts::MAX_GRAPH_BYTES {
        return Err(Error::new(
            ErrorKind::DataError,
            format!(
                "serialized graph.json is {} bytes, exceeding the {}-byte cap",
                graph_bytes.len(),
                consts::MAX_GRAPH_BYTES
            ),
        ));
    }

    let mut graph_writer = guard.writer(Path::new("graph.json"))?;
    graph_writer.write_all(&graph_bytes)?;
    graph_writer.commit()?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let manifest = Manifest {
        created_at: now,
        updated_at: now,
        commit_sha: meta.commit_sha,
        dirty: None,
        file_count,
        node_count,
        edge_count,
        redaction,
        ignore_rule_digest: meta.ignore_rule_digest,
        file_sha256: meta.file_sha256,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| {
        Error::with_source(ErrorKind::DataError, "failed to serialize manifest.json", e)
    })?;
    let mut manifest_writer = guard.writer(Path::new("manifest.json"))?;
    manifest_writer.write_all(&manifest_bytes)?;
    manifest_writer.commit()?;

    Ok(manifest)
}

fn validate_edge_endpoints(nodes: &[Node], edges: &[Edge]) -> Result<()> {
    let node_ids: BTreeSet<&NodeId> = nodes.iter().map(|n| &n.id).collect();
    for edge in edges {
        if !node_ids.contains(&edge.from) || !node_ids.contains(&edge.to) {
            return Err(Error::new(
                ErrorKind::DataError,
                format!(
                    "edge `{}` ({}) references a node that does not exist in this graph",
                    edge.id,
                    edge.kind.as_str()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Confidence, EdgeKind, FileNode};
    use crate::lang::Lang;
    use crate::taint::Provenance;

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "carto-persist-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn guard(base: &TempDir) -> PathGuard {
        let repo_root = base.path().join("repo");
        let out_root = base.path().join("out");
        std::fs::create_dir_all(&repo_root).unwrap();
        PathGuard::new(&repo_root, &out_root, false).unwrap()
    }

    fn file_node(path: &str) -> Node {
        Node::file(
            super::super::file_id(path),
            Provenance::Syntactic,
            "test@1",
            FileNode {
                path: path.to_string(),
                lang: Lang::PlainText,
                size: 0,
                sha256: None,
                skipped: None,
                excluded: None,
            },
        )
    }

    fn empty_meta() -> PersistMeta {
        PersistMeta {
            commit_sha: None,
            ignore_rule_digest: "test-digest".to_string(),
            file_sha256: BTreeMap::new(),
        }
    }

    #[test]
    fn persists_graph_json_and_manifest_json() {
        let base = TempDir::new("basic");
        let g = guard(&base);
        let mut graph = Graph::new();
        graph.insert_node(file_node("a.rs"));

        let manifest = persist(graph, empty_meta(), &g).unwrap();
        assert_eq!(manifest.node_count, 1);
        assert_eq!(manifest.edge_count, 0);
        assert!(manifest.dirty.is_none());

        let graph_json = std::fs::read_to_string(g.out_root().join("graph.json")).unwrap();
        assert!(graph_json.contains("\"schema_version\""));
        assert!(!graph_json.contains("\"created_at\"")); // no timestamps in graph.json (INV-7)

        let manifest_json = std::fs::read_to_string(g.out_root().join("manifest.json")).unwrap();
        assert!(manifest_json.contains("\"created_at\""));
    }

    #[test]
    fn manifest_file_count_counts_only_file_nodes() {
        let base = TempDir::new("filecount");
        let g = guard(&base);
        let mut graph = Graph::new();
        graph.insert_node(file_node("a.rs"));
        graph.insert_node(Node::symbol(
            super::super::sym_id("a.rs", "function", "foo", 1),
            Provenance::Syntactic,
            "lang-rust@1",
            crate::graph::SymbolNode {
                name: "foo".to_string(),
                sym_kind: crate::graph::SymKind::Function,
                file: super::super::file_id("a.rs"),
                start_line: 1,
                end_line: 2,
                signature: None,
                unresolved_calls: vec![],
                uncaptured_inbound_calls: 0,
                uncaptured_outbound_calls: 0,
                unresolved_inbound_calls: vec![],
                unresolved_inbound_call_count: 0,
            },
        ));

        let manifest = persist(graph, empty_meta(), &g).unwrap();
        assert_eq!(manifest.node_count, 2);
        assert_eq!(
            manifest.file_count, 1,
            "symbols must not inflate file_count"
        );
    }

    #[test]
    fn rejects_dangling_edge_endpoint_as_data_error() {
        let base = TempDir::new("dangling");
        let g = guard(&base);
        let mut graph = Graph::new();
        graph.insert_node(file_node("a.rs"));
        // "b.rs" is never inserted as a node.
        graph.insert_edge(Edge::new(
            EdgeKind::Imports,
            super::super::file_id("a.rs"),
            super::super::file_id("b.rs"),
            Confidence::Certain,
            "test".into(),
        ));

        let err = persist(graph, empty_meta(), &g).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DataError);
    }

    #[test]
    fn same_graph_persists_byte_identical_graph_json_twice() {
        let base_a = TempDir::new("det-a");
        let base_b = TempDir::new("det-b");
        let g_a = guard(&base_a);
        let g_b = guard(&base_b);

        let build = || {
            let mut graph = Graph::new();
            graph.insert_node(file_node("z.rs"));
            graph.insert_node(file_node("a.rs"));
            graph
        };

        persist(build(), empty_meta(), &g_a).unwrap();
        // Real-world determinism runs are further apart in time than this
        // test can be, but graph.json must not depend on created_at
        // anyway (INV-7) — sleeping isn't needed to prove that here.
        persist(build(), empty_meta(), &g_b).unwrap();

        let a = std::fs::read(g_a.out_root().join("graph.json")).unwrap();
        let b = std::fs::read(g_b.out_root().join("graph.json")).unwrap();
        assert_eq!(a, b);
    }
}
