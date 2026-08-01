//! `carto map --budget <lines>` (spec §7.1). Core produces a pre-
//! rendered, budget-capped set of lines (`carto_core::query::map`'s
//! module doc explains why); this file is the CLI wrapper: args, load,
//! print.

use crate::target;
use carto_core::consts;
use carto_core::error::Result;
use carto_core::graph;
use carto_core::query::{self, MapQuery, QueryGraph};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct MapArgs {
    /// Repo path. Defaults to the current directory.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Output directory carto previously indexed into. Defaults to the
    /// same location `carto index` uses by default.
    #[arg(long)]
    out: Option<PathBuf>,

    /// Max lines in the rendered overview.
    #[arg(long, default_value_t = consts::DEFAULT_MAP_BUDGET)]
    budget: u32,

    /// Restrict the overview to this repo-relative directory (e.g.
    /// `src/handlers`) — each section's underlying numbers stay
    /// whole-repo-accurate; only which rows are listed narrows.
    /// Independent of the positional PATH argument above, which only
    /// locates the index — PATH doesn't scope an already-built `--out`
    /// index by itself.
    #[arg(long)]
    subpath: Option<String>,
}

pub fn run(args: &MapArgs) -> Result<query::MapResult> {
    let target = target::resolve(&args.path, &args.out)?;
    let doc = graph::load(&target.out_root)?;
    let qg = QueryGraph::from_document(doc);
    let map_query = MapQuery {
        budget: args.budget,
        subpath: args.subpath.clone(),
    };
    Ok(query::map(&qg, &map_query))
}

pub fn print_human(result: &query::MapResult) {
    for line in &result.lines {
        println!("{line}");
    }
    if result.truncation.truncated {
        if let Some(next) = &result.truncation.next_call {
            println!("... truncated; see: {next}");
        }
    }
}
