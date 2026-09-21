//! `carto contract <value>` — not spec §7.1's command set (ADR-0026).
//! Core lookup lives in `carto_core::query::contract`; this file is the
//! thin CLI wrapper: args, load, print.

use crate::component_arg::parse_component_arg;
use carto_core::error::Result;
use carto_core::query::{self, ContractQuery, QueryGraph};
use carto_core::{graph, target};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct ContractArgs {
    /// Exact literal value to look up (e.g. a metric name).
    value: String,

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

    /// Max matches to return.
    #[arg(long, default_value_t = query::contract::DEFAULT_LIMIT)]
    limit: usize,

    /// Restrict listed producer/consumer sites to one of these
    /// components (repeatable). A match with no in-scope sites left
    /// still appears, with empty producer/consumer lists — that's
    /// itself informative for a cross-component contract.
    #[arg(long)]
    component: Vec<String>,
}

pub fn run(args: &ContractArgs) -> Result<query::ContractResult> {
    let target = target::resolve(&args.path, &args.out)?;
    let doc = graph::load(&target.out_root)?;
    let qg = QueryGraph::from_document(doc);
    let component = parse_component_arg(&qg, &args.component)?;
    let contract_query = ContractQuery {
        value: args.value.clone(),
        category: args.category.clone(),
        limit: args.limit,
        component,
    };
    Ok(query::contract(&qg, &contract_query))
}

pub fn print_human(result: &query::ContractResult) {
    if result.matches.is_empty() {
        println!("no matches");
        return;
    }
    for m in &result.matches {
        let qualifier = m
            .qualifier
            .as_deref()
            .map(|q| format!(" [{q}]"))
            .unwrap_or_default();
        println!("{}  ({}){qualifier}", m.value, m.category);
        println!("  producers:");
        print_sites(&m.producers);
        println!("  consumers:");
        print_sites(&m.consumers);
    }
    if result.truncation.truncated {
        if let Some(next) = &result.truncation.next_call {
            println!("... truncated; see: {next}");
        }
    }
}

fn print_sites(sites: &[query::ContractSite]) {
    if sites.is_empty() {
        println!("    (none)");
        return;
    }
    for s in sites {
        let component = s
            .component
            .as_deref()
            .map(|c| format!("  [{c}]"))
            .unwrap_or_default();
        println!(
            "    {}  ({})  {}{component}",
            s.label,
            s.confidence.as_str(),
            s.location.as_deref().unwrap_or(""),
        );
    }
}
