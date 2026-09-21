//! `carto index` — spec §7.1: "repo path, infra flags" -> "manifest
//! summary". Thin CLI wrapper (args, target resolution, print) over
//! `carto_core::indexer::build_and_persist`, which carto-mcp's `index`
//! tool calls too — no indexing logic lives here.

use carto_core::error::Result;
use carto_core::indexer::{self, IndexReport};
use carto_core::{pathguard, target};
use clap::Args;
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

pub fn run(args: &IndexArgs) -> Result<IndexReport> {
    let target = target::resolve(&args.path, &args.out)?;

    // Whether this --out counts as the spec §7.4 "explicit" in-repo
    // override: only when the user actually passed --out (never the
    // default XDG-cache path) *and* it resolves under the repo root.
    // Resolved via pathguard's own not-yet-created-path canonicalization,
    // since out_root may not exist yet and a naive canonicalize() would
    // fail.
    let prospective_out = pathguard::resolve_prospective(&target.out_root)?;
    let allow_writes_in_repo = args.out.is_some() && prospective_out.starts_with(&target.repo_root);

    indexer::build_and_persist(
        &target.repo_root,
        &target.out_root,
        !args.no_gitignore,
        allow_writes_in_repo,
    )
}

pub fn print_human(summary: &IndexReport) {
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
    println!("  components: {}", summary.component_count);
    if summary.component_count == 0 {
        println!(
            "    no nested project roots detected — a monorepo with only a top-level manifest gets none automatically; declare components in .carto/roots.json if this repo has several"
        );
    }
}
