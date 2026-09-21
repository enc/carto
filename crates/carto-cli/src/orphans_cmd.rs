//! `carto orphans` — not spec §7.1's command set (ADR-0026). Core scan
//! lives in `carto_core::query::orphans`; this file is the thin CLI
//! wrapper: args, load, print.

use crate::component_arg::parse_component_arg;
use carto_core::error::Result;
use carto_core::query::{self, OrphansQuery, QueryGraph};
use carto_core::{graph, target};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct OrphansArgs {
    /// Repo path. Defaults to the current directory.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Output directory carto previously indexed into. Defaults to the
    /// same location `carto index` uses by default.
    #[arg(long)]
    out: Option<PathBuf>,

    /// Restrict to one category (e.g. `metric_name`). Defaults to every
    /// category.
    #[arg(long)]
    category: Option<String>,

    /// Max rows per list (`consumed_never_produced` and
    /// `produced_never_consumed` are each capped independently).
    #[arg(long, default_value_t = query::orphans::DEFAULT_LIMIT)]
    limit: usize,

    /// Restrict the report to orphan contracts with at least one
    /// producer/consumer site in one of these components (repeatable)
    /// — "which of my component's contracts are orphaned."
    #[arg(long)]
    component: Vec<String>,
}

pub fn run(args: &OrphansArgs) -> Result<query::OrphansResult> {
    let target = target::resolve(&args.path, &args.out)?;
    let doc = graph::load(&target.out_root)?;
    let qg = QueryGraph::from_document(doc);
    let component = parse_component_arg(&qg, &args.component)?;
    let orphans_query = OrphansQuery {
        category: args.category.clone(),
        limit: args.limit,
        component,
    };
    Ok(query::orphans(&qg, &orphans_query))
}

pub fn print_human(result: &query::OrphansResult) {
    println!(
        "## consumed, never produced ({})",
        result.consumed_never_produced.len()
    );
    if result.consumed_never_produced.is_empty() {
        println!("  (none)");
    }
    for c in &result.consumed_never_produced {
        print_row(c);
    }

    println!(
        "## produced, never consumed ({})",
        result.produced_never_consumed.len()
    );
    if result.produced_never_consumed.is_empty() {
        println!("  (none)");
    }
    for c in &result.produced_never_consumed {
        print_row(c);
    }

    if result.truncation.truncated {
        if let Some(next) = &result.truncation.next_call {
            println!("... truncated; see: {next}");
        }
    }
}

fn print_row(c: &query::OrphanContract) {
    let qualifier = c
        .qualifier
        .as_deref()
        .map(|q| format!(" [{q}]"))
        .unwrap_or_default();
    let components = if c.components.is_empty() {
        String::new()
    } else {
        format!("  [{}]", c.components.join(", "))
    };
    println!("  {}  ({}){qualifier}{components}", c.value, c.category);
}
