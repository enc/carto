//! Shared `--component` flag parsing (ADR-0034/0035): every query
//! command (`where`/`deps`/`map`/`contract`/`orphans`) takes this
//! repeatable flag identically — a `Vec<String>` from `clap`, an empty
//! `Vec` meaning "no restriction," and validated against the index's
//! own known component names before use. Factored out once rather than
//! five near-identical copies, mirroring `carto-mcp`'s own
//! `render::parse_component_list` for the same param on the MCP side.

use carto_core::error::Result;
use carto_core::query::QueryGraph;
use std::collections::BTreeSet;

pub(crate) fn parse_component_arg(
    qg: &QueryGraph,
    values: &[String],
) -> Result<Option<BTreeSet<String>>> {
    let component = if values.is_empty() {
        None
    } else {
        Some(values.iter().cloned().collect::<BTreeSet<_>>())
    };
    qg.validate_component_filter(component.as_ref())?;
    Ok(component)
}
