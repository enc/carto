//! Whole-repo indexing: `walk` -> `lang::extract_and_resolve` -> `graph::
//! persist`. Spec §3's write-path pipeline, factored out of `carto-cli`'s
//! `index` command so `carto-mcp`'s `index` tool (spec §7.1/§7.3: "commands
//! = MCP tools, same core functions") calls the exact same implementation
//! rather than a second copy — mirrors [`crate::target`]'s own "moved out
//! of carto-cli so both front ends share it" precedent.
//!
//! [`IndexReport`] is a plain serde type for the same reason `query`'s
//! result structs are (`query/mod.rs`'s module doc): the CLI's `--json`
//! output and the MCP tool's `structuredContent` are the same struct, not
//! two shapes that could drift.

use crate::components::ComponentSet;
use crate::error::Result;
use crate::graph::Graph;
use crate::{gitinfo, graph, lang, pathguard, walk};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The manifest summary spec §7.1 names as `index`'s output.
#[derive(Serialize)]
pub struct IndexReport {
    pub out_dir: PathBuf,
    pub file_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub commit_sha: Option<String>,
    pub redaction_count: u64,
    /// ADR-0036: how many components (ADR-0034/0035) this index
    /// discovered/declared — surfaced here so a monorepo that got zero
    /// (a single top-level manifest with no `.carto/roots.json`, the
    /// single-manifest-monorepo gap ADR-0034 otherwise leaves silent)
    /// is visible right at `index` time, not only discoverable via
    /// `map --section components` after the fact.
    pub component_count: usize,
}

/// Walks `repo_root`, extracts+resolves every registered language's
/// symbols/imports/calls, and persists the result to `out_root` through
/// [`crate::pathguard`] (INV-3/INV-4). `respect_gitignore` and
/// `allow_writes_in_repo` are the two decisions each caller (CLI arg
/// parsing, MCP tool params) must make before calling in; everything
/// after that is identical regardless of front end.
pub fn build_and_persist(
    repo_root: &Path,
    out_root: &Path,
    respect_gitignore: bool,
    allow_writes_in_repo: bool,
) -> Result<IndexReport> {
    let guard = pathguard::PathGuard::new(repo_root, out_root, allow_writes_in_repo)?;

    let mut walked = walk::walk(repo_root, respect_gitignore)?;
    let file_count = walked.nodes.len();

    // Multi-root support (ADR-0034/0035): recognizes the project roots
    // inside this walked tree from `walked.nodes` itself — no second
    // traversal — then stamps each `FileNode::component` before
    // extraction/resolution runs, so `lang::extract_and_resolve` (and,
    // downstream, `resolve.rs`'s component-scoped tiers) see it on every
    // file from the start rather than needing `ComponentSet` threaded
    // through separately.
    let components = ComponentSet::discover(repo_root, &walked.nodes)?;
    for node in &mut walked.nodes {
        if let crate::graph::NodeData::File(file) = &mut node.data {
            file.component = components.component_of_path(&file.path).map(str::to_string);
        }
    }

    let resolved = lang::extract_and_resolve(repo_root, &walked.nodes, components.components())?;

    let mut g = Graph::new();
    for node in walked.nodes {
        g.insert_node(node);
    }
    for node in resolved.nodes {
        g.insert_node(node);
    }
    for edge in resolved.edges {
        g.insert_edge(edge);
    }

    let commit_sha = gitinfo::head_sha(repo_root);
    let meta = graph::PersistMeta {
        commit_sha: commit_sha.clone(),
        ignore_rule_digest: walked.ignore_rule_digest,
        file_sha256: walked.file_sha256,
        roots_rule_digest: components.config_digest().to_string(),
    };

    let component_count = components.components().len();
    let manifest = graph::persist(g, meta, components.components_sorted_by_path(), &guard)?;

    Ok(IndexReport {
        out_dir: guard.out_root().to_path_buf(),
        file_count,
        node_count: manifest.node_count,
        edge_count: manifest.edge_count,
        commit_sha,
        redaction_count: manifest.redaction.total(),
        component_count,
    })
}
