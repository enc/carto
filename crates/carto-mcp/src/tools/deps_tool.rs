//! MCP tool `deps` (spec §7.1's `deps <id|name>`), mirroring
//! `carto-cli/src/deps_cmd.rs`: same [`carto_core::query::deps`] core
//! call, same human-rendering shape (root line, unresolved-calls note,
//! per-hop edges with confidence).

use crate::render;
use carto_core::error::{Error, ErrorKind};
use carto_core::graph::EdgeKind;
use carto_core::query::{self, DepsQuery, Direction, QueryGraph};
use carto_core::{consts, graph, target};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) const PARAM_NAMES: &[&str] = &[
    "repo_path",
    "target",
    "out",
    "dir",
    "depth",
    "kinds",
    "subpath",
];

/// How many unresolved-call names the *text* rendering shows — mirrors
/// `deps_cmd.rs`'s `UNRESOLVED_CALLS_SHOWN`. `structuredContent` always
/// carries the full, uncapped list; only this text cap applies.
const UNRESOLVED_CALLS_SHOWN: usize = 20;

pub fn call(args: &Value) -> Result<Value, String> {
    let repo_path = args
        .get("repo_path")
        .and_then(Value::as_str)
        .ok_or("missing required param `repo_path`")?;
    let target_name = args
        .get("target")
        .and_then(Value::as_str)
        .ok_or("missing required param `target`")?;
    let out = args.get("out").and_then(Value::as_str).map(PathBuf::from);
    let dir = match args.get("dir").and_then(Value::as_str) {
        None | Some("out") => Direction::Out,
        Some("in") => Direction::In,
        Some("both") => Direction::Both,
        Some(other) => {
            return Err(format!(
                "invalid `dir` value `{other}` — expected in/out/both"
            ));
        }
    };
    let depth = args
        .get("depth")
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(1);
    let kinds = match args.get("kinds").and_then(Value::as_str) {
        None => None,
        Some(s) => Some(parse_kinds(s).map_err(|e| e.to_string())?),
    };
    let subpath = args
        .get("subpath")
        .and_then(Value::as_str)
        .map(str::to_string);

    let t = target::resolve(Path::new(repo_path), &out).map_err(|e| e.to_string())?;
    let doc = graph::load(&t.out_root).map_err(|e| e.to_string())?;
    let qg = QueryGraph::from_document(doc);
    let deps_query = DepsQuery {
        target: target_name.to_string(),
        dir,
        depth,
        kinds,
        subpath,
    };
    let result = query::deps(&qg, &deps_query).map_err(|e| e.to_string())?;

    let text = render_text(&result);
    let structured = serde_json::to_value(&result).map_err(|e| e.to_string())?;
    Ok(render::envelope(structured, &text))
}

fn parse_kinds(s: &str) -> carto_core::error::Result<BTreeSet<EdgeKind>> {
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
                    format!("unknown edge kind `{part}` in `kinds`"),
                ));
            }
        }
    }
    Ok(kinds)
}

fn render_text(result: &query::DepsResult) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} ({})  {}\n",
        result.root.label,
        result.root.kind,
        result.root.location.as_deref().unwrap_or("")
    ));
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
        out.push_str(&format!(
            "  ({total} unresolved call{} not shown as edges: {}{suffix})\n",
            if total == 1 { "" } else { "s" },
            names.join(", "),
        ));
    }
    for hop in &result.hops {
        for edge in &hop.edges {
            let arrow = match edge.direction {
                Direction::In => "<--",
                _ => "-->",
            };
            out.push_str(&format!(
                "  [{}] {} {} {} ({})  {}\n",
                hop.depth,
                arrow,
                edge.kind.as_str(),
                edge.node.label,
                edge.confidence.as_str(),
                edge.node.location.as_deref().unwrap_or(""),
            ));
        }
    }
    if result.truncation.truncated {
        match &result.truncation.next_call {
            Some(next) => out.push_str(&format!("... truncated; see: {next}\n")),
            None => out.push_str(&format!(
                "... truncated at the depth {} cap\n",
                consts::MAX_DEPS_DEPTH
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_target_is_a_tool_error() {
        let err = call(&serde_json::json!({ "repo_path": "." })).unwrap_err();
        assert!(err.contains("target"));
    }

    #[test]
    fn invalid_dir_is_a_tool_error() {
        let err = call(&serde_json::json!({
            "repo_path": ".", "target": "x", "dir": "sideways"
        }))
        .unwrap_err();
        assert!(err.contains("dir"));
    }
}
