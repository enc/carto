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
    "component",
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
    let component = crate::render::parse_component_list(args);

    let t = target::resolve(Path::new(repo_path), &out).map_err(|e| e.to_string())?;
    let doc = graph::load(&t.out_root).map_err(|e| e.to_string())?;
    let qg = QueryGraph::from_document(doc);
    qg.validate_component_filter(component.as_ref())
        .map_err(|e| e.to_string())?;
    let deps_query = DepsQuery {
        target: target_name.to_string(),
        dir,
        depth,
        kinds,
        subpath,
        component,
    };
    let result = query::deps(&qg, &deps_query).map_err(|e| e.to_string())?;

    let text = render_text(&result, dir);
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

fn render_text(result: &query::DepsResult, dir: Direction) -> String {
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
    // Only relevant to an inbound question — noise on --dir out, which
    // never uses this count at all.
    if result.root_uncaptured_inbound_calls > 0 && matches!(dir, Direction::In | Direction::Both) {
        out.push_str(&format!(
            "  ({} call site{} elsewhere in this repo spell this symbol's name in a shape \
             this language's extractor never attempts to resolve — not evidence of zero \
             callers; INV-8 honest omission, not a claim)\n",
            result.root_uncaptured_inbound_calls,
            if result.root_uncaptured_inbound_calls == 1 {
                ""
            } else {
                "s"
            },
        ));
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
        out.push_str(&format!(
            "  ({total} call site{} elsewhere spell this symbol's name and were attempted but \
             produced no edge (most often because the name is declared more than once — e.g. \
             an interface method and its implementation): {}{count_note}; not evidence of zero \
             callers)\n",
            if total == 1 { "" } else { "s" },
            sites.join(", "),
        ));
    }
    // Outbound counterpart (ADR-0023) — only relevant to an outbound
    // question, noise on --dir in.
    if result.root_uncaptured_outbound_calls > 0 && matches!(dir, Direction::Out | Direction::Both)
    {
        out.push_str(&format!(
            "  ({} call site{} inside this symbol's own body spell a callee name in a shape \
             this language's extractor never attempts to resolve — not evidence it calls \
             little; INV-8 honest omission, not a claim)\n",
            result.root_uncaptured_outbound_calls,
            if result.root_uncaptured_outbound_calls == 1 {
                ""
            } else {
                "s"
            },
        ));
    }
    // ADR-0044: a Terraform variable may be set outside the code, so an
    // inbound question about one always carries the caveat.
    if matches!(dir, Direction::In | Direction::Both) {
        if let Some(ext) = &result.root_may_be_set_externally {
            out.push_str(&format!("  {}\n", ext.note()));
        }
    }
    for hop in &result.hops {
        for edge in &hop.edges {
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
            out.push_str(&format!(
                "  [{}] {} {} {} ({})  {}{same_file_note}{cross_component_note}\n",
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

/// ADR-0034/0035: the `[cross-component]` marker text, or `""` — see
/// `carto-cli/src/deps_cmd.rs`'s own copy of this function for the full
/// reasoning (only worth stating for a File/Symbol target; a
/// Module/Contract node has no component concept at all).
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
    use carto_core::graph::Confidence;
    use carto_core::query::NodeSummary;

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
    fn marker_for_a_crossing_into_the_none_bucket() {
        // The exact bug this regression guards: a File/Symbol target
        // with no component at all is still a real crossing when the
        // root has one.
        assert_eq!(
            cross_component_note(&edge("file", None, false)),
            "  [cross-component]"
        );
    }

    #[test]
    fn no_marker_for_module_or_contract_targets() {
        assert_eq!(cross_component_note(&edge("module", None, false)), "");
        assert_eq!(cross_component_note(&edge("contract", None, false)), "");
    }
}
