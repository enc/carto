//! MCP tool `map` (spec §7.1's `map --budget <lines>`), mirroring
//! `carto-cli/src/map_cmd.rs`: same [`carto_core::query::map`] core call.
//! `MapResult::lines` is already pre-rendered, budget-capped text (no
//! `TaintedString` fields — file paths only, matching
//! `docs/STATUS.md`'s "redaction only scans TaintedString fields, not
//! extractor-computed paths" note), so no fencing is needed here, same as
//! the CLI's own `map_cmd.rs::print_human`.

use crate::render;
use carto_core::query::{self, MapQuery, QueryGraph};
use carto_core::{consts, graph, target};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) const PARAM_NAMES: &[&str] = &["repo_path", "out", "budget", "subpath"];

pub fn call(args: &Value) -> Result<Value, String> {
    let repo_path = args
        .get("repo_path")
        .and_then(Value::as_str)
        .ok_or("missing required param `repo_path`")?;
    let out = args.get("out").and_then(Value::as_str).map(PathBuf::from);
    let budget = args
        .get("budget")
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(consts::DEFAULT_MAP_BUDGET);
    let subpath = args
        .get("subpath")
        .and_then(Value::as_str)
        .map(str::to_string);

    let t = target::resolve(Path::new(repo_path), &out).map_err(|e| e.to_string())?;
    let doc = graph::load(&t.out_root).map_err(|e| e.to_string())?;
    let qg = QueryGraph::from_document(doc);
    let result = query::map(&qg, &MapQuery { budget, subpath });

    let text = render_text(&result);
    let structured = serde_json::to_value(&result).map_err(|e| e.to_string())?;
    Ok(render::envelope(structured, &text))
}

fn render_text(result: &query::MapResult) -> String {
    let mut out = result.lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if result.truncation.truncated {
        if let Some(next) = &result.truncation.next_call {
            out.push_str(&format!("... truncated; see: {next}\n"));
        }
    }
    out
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
