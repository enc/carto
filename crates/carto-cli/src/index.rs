//! `carto index` — spec §7.1: "repo path, infra flags" -> "manifest
//! summary". `File` nodes via `walk`; `Symbol`/`Module` nodes and
//! `contains`/`imports`/`calls` edges via `lang::extract_and_resolve`
//! for every walked file whose language has a registered extractor
//! (Rust only as of M1.b.2a). No infra flags yet (M2).

use crate::target;
use carto_core::error::Result;
use carto_core::graph::Graph;
use carto_core::{gitinfo, graph, lang, pathguard, walk};
use clap::Args;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Args)]
pub struct IndexArgs {
    /// Repo path to index. Defaults to the current directory.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Output directory. Defaults to
    /// `${XDG_CACHE_HOME:-~/.cache}/carto/<repo-hash>/` (INV-3). Pointing
    /// this inside the repo is the only way to opt into in-repo writes
    /// (spec §7.4) — otherwise any resolved path under the repo root is
    /// refused, exit code 3.
    #[arg(long)]
    out: Option<PathBuf>,

    /// Disable respecting .gitignore/.git/info/exclude/global gitignore.
    /// The built-in denylist and .cartoignore still apply regardless
    /// (spec §5.1).
    #[arg(long)]
    no_gitignore: bool,
}

/// The manifest summary spec §7.1 names as `index`'s output — human by
/// default, this same struct under `--json`.
#[derive(Serialize)]
pub struct IndexSummary {
    pub out_dir: PathBuf,
    pub file_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub commit_sha: Option<String>,
    pub redaction_count: u64,
}

pub fn run(args: &IndexArgs) -> Result<IndexSummary> {
    let target = target::resolve(&args.path, &args.out)?;
    let repo_root = target.repo_root;
    let out_root = target.out_root;

    // Whether this --out counts as the spec §7.4 "explicit" in-repo
    // override: only when the user actually passed --out (never the
    // default XDG-cache path) *and* it resolves under the repo root.
    // Resolved via pathguard's own not-yet-created-path canonicalization,
    // since out_root may not exist yet and a naive canonicalize() would
    // fail.
    let prospective_out = pathguard::resolve_prospective(&out_root)?;
    let allow_writes_in_repo = args.out.is_some() && prospective_out.starts_with(&repo_root);

    let guard = pathguard::PathGuard::new(&repo_root, &out_root, allow_writes_in_repo)?;

    let walked = walk::walk(&repo_root, !args.no_gitignore)?;
    let file_count = walked.nodes.len();

    let resolved = lang::extract_and_resolve(&repo_root, &walked.nodes);

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

    let commit_sha = gitinfo::head_sha(&repo_root);
    let meta = graph::PersistMeta {
        commit_sha: commit_sha.clone(),
        ignore_rule_digest: walked.ignore_rule_digest,
        file_sha256: walked.file_sha256,
    };

    let manifest = graph::persist(g, meta, &guard)?;

    Ok(IndexSummary {
        out_dir: guard.out_root().to_path_buf(),
        file_count,
        node_count: manifest.node_count,
        edge_count: manifest.edge_count,
        commit_sha,
        redaction_count: manifest.redaction.total(),
    })
}

pub fn print_human(summary: &IndexSummary) {
    println!("indexed {} files", summary.file_count);
    println!("  out dir: {}", summary.out_dir.display());
    println!("  nodes:   {}", summary.node_count);
    println!("  edges:   {}", summary.edge_count);
    match &summary.commit_sha {
        Some(sha) => println!("  commit:  {sha}"),
        None => println!("  commit:  <unknown>"),
    }
    if summary.redaction_count > 0 {
        println!("  redactions: {}", summary.redaction_count);
    }
}
