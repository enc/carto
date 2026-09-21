//! MCP tool `contract` (ADR-0026 — not spec §7.1's tool set), mirroring
//! `carto-cli/src/contract_cmd.rs`: same [`carto_core::query::contract`]
//! core call.

use crate::render;
use carto_core::query::{self, ContractQuery, QueryGraph};
use carto_core::{graph, target};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) const PARAM_NAMES: &[&str] = &[
    "repo_path",
    "value",
    "out",
    "category",
    "limit",
    "component",
];

pub fn call(args: &Value) -> Result<Value, String> {
    let repo_path = args
        .get("repo_path")
        .and_then(Value::as_str)
        .ok_or("missing required param `repo_path`")?;
    let value = args
        .get("value")
        .and_then(Value::as_str)
        .ok_or("missing required param `value`")?;
    let out = args.get("out").and_then(Value::as_str).map(PathBuf::from);
    let category = args
        .get("category")
        .and_then(Value::as_str)
        .map(str::to_string);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(query::contract::DEFAULT_LIMIT);
    let component = crate::render::parse_component_list(args);

    let t = target::resolve(Path::new(repo_path), &out).map_err(|e| e.to_string())?;
    let doc = graph::load(&t.out_root).map_err(|e| e.to_string())?;
    let qg = QueryGraph::from_document(doc);
    qg.validate_component_filter(component.as_ref())
        .map_err(|e| e.to_string())?;
    let contract_query = ContractQuery {
        value: value.to_string(),
        category,
        limit,
        component,
    };
    let result = query::contract(&qg, &contract_query);

    let text = render_text(&result);
    let structured = serde_json::to_value(&result).map_err(|e| e.to_string())?;
    Ok(render::envelope(structured, &text))
}

fn render_text(result: &query::ContractResult) -> String {
    if result.matches.is_empty() {
        return "no matches".to_string();
    }
    let mut out = String::new();
    for m in &result.matches {
        let qualifier = m
            .qualifier
            .as_deref()
            .map(|q| format!(" [{q}]"))
            .unwrap_or_default();
        out.push_str(&format!("{}  ({}){qualifier}\n", m.value, m.category));
        out.push_str("  producers:\n");
        render_sites(&mut out, &m.producers);
        out.push_str("  consumers:\n");
        render_sites(&mut out, &m.consumers);
    }
    if result.truncation.truncated {
        if let Some(next) = &result.truncation.next_call {
            out.push_str(&format!("... truncated; see: {next}\n"));
        }
    }
    out
}

fn render_sites(out: &mut String, sites: &[query::ContractSite]) {
    if sites.is_empty() {
        out.push_str("    (none)\n");
        return;
    }
    for s in sites {
        let component = s
            .component
            .as_deref()
            .map(|c| format!("  [{c}]"))
            .unwrap_or_default();
        out.push_str(&format!(
            "    {}  ({})  {}{component}\n",
            s.label,
            s.confidence.as_str(),
            s.location.as_deref().unwrap_or(""),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_value_is_a_tool_error() {
        let err = call(&serde_json::json!({ "repo_path": "." })).unwrap_err();
        assert!(err.contains("value"));
    }
}
