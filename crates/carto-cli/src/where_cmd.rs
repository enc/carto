//! `carto where <name>` (spec §7.1). Named `where_cmd` in this crate
//! because `where` is a Rust keyword and can't name a module without
//! `r#where` friction — the clap subcommand itself is still named
//! `where` (`#[command(name = "where")]` in `main.rs`). Core matching
//! logic lives in `carto_core::query::find`; this file is the thin CLI
//! wrapper: args, load, print.

use crate::component_arg::parse_component_arg;
use carto_core::error::Result;
use carto_core::query::{self, FindQuery, QueryGraph};
use carto_core::taint::TaintedString;
use carto_core::{consts, graph, target};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct WhereArgs {
    /// Symbol name to search for (case-insensitive substring by default).
    needle: String,

    /// Repo path. Defaults to the current directory.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Output directory carto previously indexed into. Defaults to the
    /// same location `carto index` uses by default.
    #[arg(long)]
    out: Option<PathBuf>,

    /// Exact, case-sensitive match instead of case-insensitive substring.
    #[arg(long)]
    exact: bool,

    /// Max matches to return.
    #[arg(long, default_value_t = query::find::DEFAULT_LIMIT)]
    limit: usize,

    /// Restrict matches to symbols under this repo-relative directory
    /// (e.g. `src/handlers`). Independent of the positional PATH
    /// argument above, which only locates the index — PATH doesn't
    /// scope an already-built `--out` index by itself.
    #[arg(long)]
    subpath: Option<String>,

    /// Restrict matches to one of these components (repeatable, e.g.
    /// `--component orders --component billing`). A component is a
    /// project root carto detected inside the repo (or one declared in
    /// `.carto/roots.json`) — see `carto map --section components`.
    /// Independent of `--subpath`; both given means both apply.
    #[arg(long)]
    component: Vec<String>,
}

pub fn run(args: &WhereArgs) -> Result<query::FindResult> {
    let target = target::resolve(&args.path, &args.out)?;
    let doc = graph::load(&target.out_root)?;
    let qg = QueryGraph::from_document(doc);
    let component = parse_component_arg(&qg, &args.component)?;
    let find_query = FindQuery {
        needle: args.needle.clone(),
        exact: args.exact,
        limit: args.limit,
        subpath: args.subpath.clone(),
        component,
    };
    Ok(query::find(&qg, &find_query))
}

pub fn print_human(result: &query::FindResult) {
    if result.matches.is_empty() && result.module_matches.is_empty() {
        println!("no matches");
        return;
    }

    // §8.4: tainted text (signatures) is fenced. One fence around the
    // whole listing rather than one per row — the fence delimits a
    // region of repo-derived text, and this is one region (see
    // docs/adr/0010-tainted-text-in-query-output.md).
    let has_signature = result.matches.iter().any(|m| m.signature.is_some());
    if has_signature {
        println!("{}", consts::FENCE_OPEN);
    }
    for m in &result.matches {
        let component = m
            .component
            .as_deref()
            .map(|c| format!("  [{c}]"))
            .unwrap_or_default();
        println!(
            "{}  {}  {}{component}",
            m.name,
            m.sym_kind.as_str(),
            m.location
        );
        if let Some(sig) = &m.signature {
            println!("    {}", capped(sig));
        }
    }
    if has_signature {
        println!("{}", consts::FENCE_CLOSE);
    }

    // Modules carry no tainted text (a `path` is extractor-computed, not
    // captured source), so no fence is needed here.
    if !result.module_matches.is_empty() {
        println!("## modules");
        for m in &result.module_matches {
            println!(
                "{}  {}  {}",
                m.path,
                if m.external { "external" } else { "internal" },
                m.id
            );
        }
    }

    if result.truncation.truncated {
        if let Some(next) = &result.truncation.next_call {
            println!("... truncated; see: {next}");
        }
    }
}

fn capped(sig: &TaintedString) -> String {
    sig.render_capped(consts::SIGNATURE_CAP)
}
