//! MCP tool `orphans` (ADR-0026 — not spec §7.1's tool set), mirroring
//! `carto-cli/src/orphans_cmd.rs`: same [`carto_core::query::orphans`]
//! core call.

use crate::render;
use carto_core::query::{self, OrphansQuery, QueryGraph};
use carto_core::{graph, target};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) const PARAM_NAMES: &[&str] = &["repo_path", "out", "category", "limit", "component"];

pub fn call(args: &Value) -> Result<Value, String> {
    let repo_path = args
        .get("repo_path")
        .and_then(Value::as_str)
        .ok_or("missing required param `repo_path`")?;
    let out = args.get("out").and_then(Value::as_str).map(PathBuf::from);
    let category = args
        .get("category")
        .and_then(Value::as_str)
        .map(str::to_string);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(query::orphans::DEFAULT_LIMIT);
    let component = crate::render::parse_component_list(args);

    let t = target::resolve(Path::new(repo_path), &out).map_err(|e| e.to_string())?;
    let doc = graph::load(&t.out_root).map_err(|e| e.to_string())?;
    let qg = QueryGraph::from_document(doc);
    qg.validate_component_filter(component.as_ref())
        .map_err(|e| e.to_string())?;
    let orphans_query = OrphansQuery {
        category,
        limit,
        component,
    };
    let result = query::orphans(&qg, &orphans_query);

    let text = render_text(&result);
    let structured = serde_json::to_value(&result).map_err(|e| e.to_string())?;
    Ok(render::envelope(structured, &text))
}

fn render_text(result: &query::OrphansResult) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "## consumed, never produced ({})\n",
        result.consumed_never_produced.len()
    ));
    if result.consumed_never_produced.is_empty() {
        out.push_str("  (none)\n");
    }
    for c in &result.consumed_never_produced {
        render_row(&mut out, c);
    }
    out.push_str(&format!(
        "## produced, never consumed ({})\n",
        result.produced_never_consumed.len()
    ));
    if result.produced_never_consumed.is_empty() {
        out.push_str("  (none)\n");
    }
    for c in &result.produced_never_consumed {
        render_row(&mut out, c);
    }
    if result.truncation.truncated {
        if let Some(next) = &result.truncation.next_call {
            out.push_str(&format!("... truncated; see: {next}\n"));
        }
    }
    out
}

fn render_row(out: &mut String, c: &query::OrphanContract) {
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
    out.push_str(&format!(
        "  {}  ({}){qualifier}{components}\n",
        c.value, c.category
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_repo_path_is_a_tool_error() {
        let err = call(&serde_json::json!({})).unwrap_err();
        assert!(err.contains("repo_path"));
    }
}
