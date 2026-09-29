//! `carto deps <target>` (spec §7.1). Core BFS lives in
//! `carto_core::query::deps`; this file is the CLI wrapper: args, load,
//! print. Named `deps_cmd` for symmetry with `where_cmd`, though `deps`
//! isn't itself a Rust keyword — keeping both command files named
//! `<name>_cmd` avoids a `where`-only special case.

use crate::component_arg::parse_component_arg;
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

    /// Restrict reported rows to one of these components (repeatable,
    /// e.g. `--component orders --component billing`) — see
    /// `carto map --section components`. Independent of `--subpath`;
    /// both given means both apply.
    #[arg(long)]
    component: Vec<String>,
}

pub fn run(args: &DepsArgs) -> Result<query::DepsResult> {
    let target = target::resolve(&args.path, &args.out)?;
    let doc = graph::load(&target.out_root)?;
    let qg = QueryGraph::from_document(doc);

    let kinds = match &args.kinds {
        None => None,
        Some(s) => Some(parse_kinds(s)?),
    };
    let component = parse_component_arg(&qg, &args.component)?;

    let deps_query = DepsQuery {
        target: args.target.clone(),
        dir: args.dir.into(),
        depth: args.depth,
        kinds,
        subpath: args.subpath.clone(),
        component,
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
    // ADR-0044: a Terraform variable may be set outside the code, so an
    // inbound question about one always carries the caveat.
    if matches!(dir, Direction::In | Direction::Both) {
        if let Some(ext) = &result.root_may_be_set_externally {
            println!("  {}", ext.note());
        }
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
            let cross_component_note = cross_component_note(edge);
            println!(
                "  [{}] {} {} {} ({})  {}{same_file_note}{cross_component_note}",
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

/// ADR-0034/0035: the `[cross-component]` marker text, or `""`. Only
/// worth stating for a File/Symbol target — a Module/Contract node has
/// no component *concept* at all, not merely a missing one, so
/// `same_component_as_root`'s own `false` there isn't "crossing a
/// boundary," it's "no boundary to cross." Checking the target's own
/// *kind* rather than whether it happens to carry a component keeps a
/// genuine crossing into the `None` bucket (a File/Symbol with no
/// component, reached from one that has one) correctly flagged too.
fn cross_component_note(edge: &query::DepEdge) -> &'static str {
    if matches!(edge.node.kind.as_str(), "file" | "symbol") && !edge.same_component_as_root {
        "  [cross-component]"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carto_core::graph::{Confidence, EdgeKind};
    use carto_core::query::NodeSummary;

    fn edge(kind: &str, component: Option<&str>, same_component_as_root: bool) -> query::DepEdge {
        query::DepEdge {
            kind: EdgeKind::Calls,
            confidence: Confidence::Inferred,
            evidence: vec![],
            direction: Direction::Out,
            node: NodeSummary {
                id: carto_core::graph::file_id("x"),
                kind: kind.to_string(),
                label: "x".to_string(),
                location: None,
                component: component.map(str::to_string),
            },
            same_file_as_root: false,
            same_component_as_root,
        }
    }

    #[test]
    fn no_marker_when_same_component() {
        assert_eq!(
            cross_component_note(&edge("symbol", Some("orders"), true)),
            ""
        );
    }

    #[test]
    fn marker_for_a_real_cross_component_symbol() {
        assert_eq!(
            cross_component_note(&edge("symbol", Some("shared"), false)),
            "  [cross-component]"
        );
    }

    #[test]
    fn marker_for_a_crossing_into_the_none_bucket() {
        // The exact bug this regression guards: a File/Symbol target
        // with no component at all is still a real crossing when the
        // root has one -- `same_component_as_root` is correctly
        // `false`, and the marker must not be suppressed just because
        // `node.component` itself happens to be `None`.
        assert_eq!(
            cross_component_note(&edge("file", None, false)),
            "  [cross-component]"
        );
        assert_eq!(
            cross_component_note(&edge("symbol", None, false)),
            "  [cross-component]"
        );
    }

    #[test]
    fn no_marker_for_module_or_contract_targets() {
        // A Module/Contract node has no component concept at all --
        // `same_component_as_root` is always `false` for them (deps.rs's
        // own gating), which must not be read as "crossing a boundary."
        assert_eq!(cross_component_note(&edge("module", None, false)), "");
        assert_eq!(cross_component_note(&edge("contract", None, false)), "");
    }
}
