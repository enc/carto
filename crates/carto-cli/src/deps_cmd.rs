//! `carto deps <target>` (spec §7.1). Core BFS lives in
//! `carto_core::query::deps`; this file is the CLI wrapper: args, load,
//! print. Named `deps_cmd` for symmetry with `where_cmd`, though `deps`
//! isn't itself a Rust keyword — keeping both command files named
//! `<name>_cmd` avoids a `where`-only special case.

use crate::target;
use carto_core::error::{Error, ErrorKind, Result};
use carto_core::graph::EdgeKind;
use carto_core::query::{self, DepsQuery, Direction, QueryGraph};
use carto_core::{consts, graph};
use clap::Args;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Copy, clap::ValueEnum)]
enum DirArg {
    In,
    Out,
    Both,
}

impl From<DirArg> for Direction {
    fn from(d: DirArg) -> Self {
        match d {
            DirArg::In => Direction::In,
            DirArg::Out => Direction::Out,
            DirArg::Both => Direction::Both,
        }
    }
}

#[derive(Args)]
pub struct DepsArgs {
    /// Node ID or exact symbol name to start from.
    target: String,

    /// Repo path. Defaults to the current directory.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Output directory carto previously indexed into. Defaults to the
    /// same location `carto index` uses by default.
    #[arg(long)]
    out: Option<PathBuf>,

    /// Which direction to follow edges: `out` (what this depends on,
    /// default), `in` (what depends on this), or `both`.
    #[arg(long, value_enum, default_value_t = DirArg::Out)]
    dir: DirArg,

    /// How many hops to traverse (1..=5).
    #[arg(long, default_value_t = 1)]
    depth: u32,

    /// Comma-separated edge kinds to include (e.g.
    /// `contains,imports,calls`). Defaults to every kind.
    #[arg(long)]
    kinds: Option<String>,
}

pub fn run(args: &DepsArgs) -> Result<query::DepsResult> {
    let target = target::resolve(&args.path, &args.out)?;
    let doc = graph::load(&target.out_root)?;
    let qg = QueryGraph::from_document(doc);

    let kinds = match &args.kinds {
        None => None,
        Some(s) => Some(parse_kinds(s)?),
    };

    let deps_query = DepsQuery {
        target: args.target.clone(),
        dir: args.dir.into(),
        depth: args.depth,
        kinds,
    };
    query::deps(&qg, &deps_query)
}

fn parse_kinds(s: &str) -> Result<BTreeSet<EdgeKind>> {
    let mut kinds = BTreeSet::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match EdgeKind::parse(part) {
            Some(k) => {
                kinds.insert(k);
            }
            None => {
                return Err(Error::new(
                    ErrorKind::UserError,
                    format!("unknown edge kind `{part}` in --kinds"),
                ));
            }
        }
    }
    Ok(kinds)
}

pub fn print_human(result: &query::DepsResult) {
    println!(
        "{} ({})  {}",
        result.root.label,
        result.root.kind,
        result.root.location.as_deref().unwrap_or("")
    );
    for hop in &result.hops {
        for edge in &hop.edges {
            // `direction` is always In or Out for a specific edge (never
            // Both — that's the query-wide filter, not a per-edge value);
            // the arrow shows which way this particular edge points.
            let arrow = match edge.direction {
                Direction::In => "<--",
                _ => "-->",
            };
            println!(
                "  [{}] {} {} {} ({})  {}",
                hop.depth,
                arrow,
                edge.kind.as_str(),
                edge.node.label,
                edge.confidence.as_str(),
                edge.node.location.as_deref().unwrap_or(""),
            );
        }
    }
    if result.truncation.truncated {
        match &result.truncation.next_call {
            Some(next) => println!("... truncated; see: {next}"),
            None => println!(
                "... truncated at the --depth {} cap",
                consts::MAX_DEPS_DEPTH
            ),
        }
    }
}
