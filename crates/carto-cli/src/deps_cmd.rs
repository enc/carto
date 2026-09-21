//! `carto deps <target>` (spec §7.1). Core BFS lives in
//! `carto_core::query::deps`; this file is the CLI wrapper: args, load,
//! print. Named `deps_cmd` for symmetry with `where_cmd`, though `deps`
//! isn't itself a Rust keyword — keeping both command files named
//! `<name>_cmd` avoids a `where`-only special case.

use carto_core::error::{Error, ErrorKind, Result};
use carto_core::graph::EdgeKind;
use carto_core::query::{self, DepsQuery, Direction, QueryGraph};
use carto_core::{consts, graph, target};
use clap::Args;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum DirArg {
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
    pub(crate) dir: DirArg,

    /// How many hops to traverse (1..=5).
    #[arg(long, default_value_t = 1)]
    depth: u32,

    /// Comma-separated edge kinds to include (e.g.
    /// `contains,imports,calls,references`). Defaults to every kind.
    #[arg(long)]
    kinds: Option<String>,

    /// Restrict reported rows to nodes under this repo-relative
    /// directory (e.g. `src/handlers`). The traversal itself still
    /// crosses out of the subtree and back if that's where the real
    /// dependency chain goes — only which rows get shown is
    /// restricted. Independent of the positional PATH argument above,
    /// which only locates the index.
    #[arg(long)]
    subpath: Option<String>,
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
        subpath: args.subpath.clone(),
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

/// Human-rendered cap on how many unresolved-call names are listed —
/// the structured/`--json` field (`DepsResult::root_unresolved_calls`)
/// always carries the full, uncapped list; only this rendering caps it,
/// same "structured field exact, rendered text capped" split `map`'s
/// `counts` vs. `lines` already uses.
const UNRESOLVED_CALLS_SHOWN: usize = 20;

pub fn print_human(result: &query::DepsResult, dir: Direction) {
    println!(
        "{} ({})  {}",
        result.root.label,
        result.root.kind,
        result.root.location.as_deref().unwrap_or("")
    );
    if !result.root_unresolved_calls.is_empty() {
        let total = result.root_unresolved_calls.len();
        let names: Vec<&str> = result
            .root_unresolved_calls
            .iter()
            .take(UNRESOLVED_CALLS_SHOWN)
            .map(|c| c.name.as_str())
            .collect();
        let suffix = if total > UNRESOLVED_CALLS_SHOWN {
            format!(", +{} more", total - UNRESOLVED_CALLS_SHOWN)
        } else {
            String::new()
        };
        println!(
            "  ({total} unresolved call{} not shown as edges: {}{suffix})",
            if total == 1 { "" } else { "s" },
            names.join(", "),
        );
    }
    // Only relevant to an inbound question — noise on --dir out, which
    // never uses this count at all.
    if result.root_uncaptured_inbound_calls > 0 && matches!(dir, Direction::In | Direction::Both) {
        println!(
            "  ({} call site{} elsewhere in this repo spell this symbol's name in a shape \
             this language's extractor never attempts to resolve — not evidence of zero \
             callers; INV-8 honest omission, not a claim)",
            result.root_uncaptured_inbound_calls,
            if result.root_uncaptured_inbound_calls == 1 {
                ""
            } else {
                "s"
            },
        );
    }
    // ADR-0033's third honesty signal: sites that spell this symbol's
    // name and *were* attempted, unlike `root_uncaptured_inbound_calls`
    // above (never-attempted syntax). Also inbound-only, so gated the
    // same way.
    if !result.root_unresolved_inbound_calls.is_empty()
        && matches!(dir, Direction::In | Direction::Both)
    {
        let total = result.root_unresolved_inbound_call_count;
        let shown = result.root_unresolved_inbound_calls.len() as u32;
        let sites: Vec<String> = result
            .root_unresolved_inbound_calls
            .iter()
            .map(|s| format!("{}:{}", s.file, s.line))
            .collect();
        let count_note = if total > shown {
            format!(" (first {shown} of {total})")
        } else {
            String::new()
        };
        println!(
            "  ({total} call site{} elsewhere spell this symbol's name and were attempted but \
             produced no edge (most often because the name is declared more than once — e.g. \
             an interface method and its implementation): {}{count_note}; not evidence of zero \
             callers)",
            if total == 1 { "" } else { "s" },
            sites.join(", "),
        );
    }
    // Outbound counterpart (ADR-0023) — only relevant to an outbound
    // question, noise on --dir in.
    if result.root_uncaptured_outbound_calls > 0 && matches!(dir, Direction::Out | Direction::Both)
    {
        println!(
            "  ({} call site{} inside this symbol's own body spell a callee name in a shape \
             this language's extractor never attempts to resolve — not evidence it calls \
             little; INV-8 honest omission, not a claim)",
            result.root_uncaptured_outbound_calls,
            if result.root_uncaptured_outbound_calls == 1 {
                ""
            } else {
                "s"
            },
        );
    }
    for hop in &result.hops {
        for edge in &hop.edges {
            // `direction` is always In or Out for a specific edge (never
            // Both — that's the query-wide filter, not a per-edge value);
            // the arrow shows which way this particular edge points.
            let arrow = match edge.direction {
                Direction::In => "<--",
                _ => "-->",
            };
            // T5: a caller in the same file as the callee reads as "the
            // definition itself" without this marker — see
            // `same_file_as_root`'s own doc comment.
            let same_file_note = if edge.same_file_as_root {
                "  [same file as root]"
            } else {
                ""
            };
            println!(
                "  [{}] {} {} {} ({})  {}{same_file_note}",
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
