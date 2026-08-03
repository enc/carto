//! Reads back what [`super::persist`] wrote (spec §4.4). Symmetric with
//! `persist` but deliberately not routed through [`crate::pathguard`] —
//! INV-3/INV-4 constrain *writes*; reading a `graph.json` the user points
//! carto at has no confinement story to enforce.
//!
//! Order matters here the same way it does in `persist`: check the file's
//! size on disk *before* reading its bytes into memory, so the §4.4 cap
//! ("refuse to load a graph.json > 512 MiB") actually bounds peak memory
//! rather than being checked after the fact.

use super::GraphDocument;
use crate::consts;
use crate::error::{Error, ErrorKind, Result};
use std::path::Path;

/// Loads and validates `<out_root>/graph.json`. Fails with:
///
/// - `UserError` (exit 1) if the file doesn't exist — nothing is corrupt,
///   the repo just hasn't been indexed yet; the message names the fix
///   (`carto index`).
/// - `DataError` (exit 2) if it's oversized, unparseable, or written by an
///   incompatible schema version.
pub fn load(out_root: &Path) -> Result<GraphDocument> {
    let path = out_root.join("graph.json");

    let metadata = std::fs::metadata(&path).map_err(|e| {
        Error::with_source(
            ErrorKind::UserError,
            format!(
                "no graph.json in `{}` — run `carto index` on this repo first",
                out_root.display()
            ),
            e,
        )
    })?;

    if metadata.len() > consts::MAX_GRAPH_BYTES {
        return Err(Error::new(
            ErrorKind::DataError,
            format!(
                "`{}` is {} bytes, exceeding the {}-byte cap",
                path.display(),
                metadata.len(),
                consts::MAX_GRAPH_BYTES
            ),
        ));
    }

    let bytes = std::fs::read(&path).map_err(|e| {
        Error::with_source(
            ErrorKind::DataError,
            format!("failed to read `{}`", path.display()),
            e,
        )
    })?;

    let doc: GraphDocument = serde_json::from_slice(&bytes).map_err(|e| {
        Error::with_source(
            ErrorKind::DataError,
            format!("failed to parse `{}`", path.display()),
            e,
        )
    })?;

    if doc.schema_version != consts::SCHEMA_VERSION {
        return Err(Error::new(
            ErrorKind::DataError,
            format!(
                "`{}` has schema_version {}, but this build of carto expects {} — re-run `carto index`",
                path.display(),
                doc.schema_version,
                consts::SCHEMA_VERSION
            ),
        ));
    }

    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Confidence, Edge, EdgeKind, FileNode, Graph, Node, PersistMeta};
    use crate::lang::Lang;
    use crate::pathguard::PathGuard;
    use crate::taint::Provenance;
    use std::collections::BTreeMap;

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "carto-load-test-{tag}-{}-{:?}",
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
    fn round_trips_a_graph_with_every_node_and_edge_kind() {
        let base = TempDir::new("roundtrip");
        let g = guard(&base);

        let mut graph = Graph::new();
        let a = super::super::file_id("a.rs");
        let b = super::super::file_id("b.rs");
        graph.insert_node(file_node("a.rs"));
        graph.insert_node(file_node("b.rs"));
        graph.insert_node(Node::symbol(
            super::super::sym_id("a.rs", "function", "foo", 1),
            Provenance::Syntactic,
            "lang-rust@1",
            crate::graph::SymbolNode {
                name: "foo".to_string(),
                sym_kind: crate::graph::SymKind::Function,
                file: a.clone(),
                start_line: 1,
                end_line: 3,
                signature: Some(crate::taint::TaintedString::new(
                    "fn foo()",
                    Provenance::Syntactic,
                )),
                unresolved_calls: vec![],
                uncaptured_inbound_calls: 0,
            },
        ));
        graph.insert_node(Node::module(
            super::super::module_id("serde", true),
            Provenance::Syntactic,
            "lang-rust@1",
            crate::graph::ModuleNode {
                path: "serde".to_string(),
                external: true,
            },
        ));
        graph.insert_edge(Edge::new(
            EdgeKind::Imports,
            a,
            b,
            Confidence::Certain,
            "mod-declaration".to_string(),
        ));

        let persisted = crate::graph::persist(graph, empty_meta(), &g).unwrap();
        let loaded = load(g.out_root()).unwrap();

        assert_eq!(loaded.nodes.len(), persisted.node_count);
        assert_eq!(loaded.edges.len(), persisted.edge_count);
        assert!(
            loaded
                .nodes
                .iter()
                .any(|n| matches!(&n.data, crate::graph::NodeData::Symbol(s) if s.name == "foo"))
        );
        assert!(
            loaded
                .nodes
                .iter()
                .any(|n| matches!(&n.data, crate::graph::NodeData::Module(m) if m.path == "serde"))
        );
        // Provenance on the node itself survives the round trip even
        // though the TaintedString field's own internal provenance is
        // re-tagged Ingested on deserialize (taint.rs's documented
        // behavior, not a bug here) — query code must read provenance
        // from Node.provenance, never TaintedString::provenance().
        let sym = loaded
            .nodes
            .iter()
            .find(|n| matches!(&n.data, crate::graph::NodeData::Symbol(s) if s.name == "foo"))
            .unwrap();
        assert_eq!(sym.provenance, Provenance::Syntactic);
    }

    #[test]
    fn missing_file_is_a_user_error_naming_index() {
        let base = TempDir::new("missing");
        let err = load(base.path()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UserError);
        assert!(format!("{err}").contains("carto index"));
    }

    #[test]
    fn garbage_json_is_a_data_error() {
        let base = TempDir::new("garbage");
        std::fs::write(base.path().join("graph.json"), b"not json").unwrap();
        let err = load(base.path()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DataError);
    }

    #[test]
    fn oversized_file_is_a_data_error_without_being_read_into_memory() {
        let base = TempDir::new("oversized");
        let path = base.path().join("graph.json");
        // A sparse file past the cap: proves the size check runs before
        // any attempt to parse the contents, without needing to actually
        // write 512 MiB of real bytes.
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(consts::MAX_GRAPH_BYTES + 1).unwrap();
        let err = load(base.path()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DataError);
    }

    #[test]
    fn wrong_schema_version_is_a_data_error() {
        let base = TempDir::new("schema");
        let doc = GraphDocument {
            carto_version: "0.1.0".to_string(),
            schema_version: consts::SCHEMA_VERSION + 1,
            nodes: vec![],
            edges: vec![],
        };
        std::fs::write(
            base.path().join("graph.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
        let err = load(base.path()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DataError);
    }
}
