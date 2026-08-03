//! MCP tool `where` (spec §7.1's `where <name>`), mirroring
//! `carto-cli/src/where_cmd.rs`: same [`carto_core::query::find`] core
//! call, same fencing discipline (ADR-0010: tainted `signature` text
//! fenced once for the whole listing, not once per row).

use crate::render;
use carto_core::query::{self, FindQuery, QueryGraph};
use carto_core::taint::TaintedString;
use carto_core::{consts, graph, target};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Every property name here must be one this file actually reads —
/// `tools/mod.rs`'s schema-drift test pins this list against
/// `schema.rs`'s `where` inputSchema. Test-only: not part of this
/// module's real behavior, so it doesn't exist in a non-test build.
#[cfg(test)]
pub(crate) const PARAM_NAMES: &[&str] =
    &["repo_path", "needle", "out", "exact", "limit", "subpath"];

pub fn call(args: &Value) -> Result<Value, String> {
    let repo_path = args
        .get("repo_path")
        .and_then(Value::as_str)
        .ok_or("missing required param `repo_path`")?;
    let needle = args
        .get("needle")
        .and_then(Value::as_str)
        .ok_or("missing required param `needle`")?;
    let out = args.get("out").and_then(Value::as_str).map(PathBuf::from);
    let exact = args.get("exact").and_then(Value::as_bool).unwrap_or(false);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(query::find::DEFAULT_LIMIT);
    let subpath = args
        .get("subpath")
        .and_then(Value::as_str)
        .map(str::to_string);

    let t = target::resolve(Path::new(repo_path), &out).map_err(|e| e.to_string())?;
    let doc = graph::load(&t.out_root).map_err(|e| e.to_string())?;
    let qg = QueryGraph::from_document(doc);
    let find_query = FindQuery {
        needle: needle.to_string(),
        exact,
        limit,
        subpath,
    };
    let result = query::find(&qg, &find_query);

    let text = render_text(&result);
    let structured = serde_json::to_value(&result).map_err(|e| e.to_string())?;
    Ok(render::envelope(structured, &text))
}

fn render_text(result: &query::FindResult) -> String {
    if result.matches.is_empty() && result.module_matches.is_empty() {
        return "no matches".to_string();
    }

    let mut out = String::new();
    let has_signature = result.matches.iter().any(|m| m.signature.is_some());
    if has_signature {
        out.push_str(consts::FENCE_OPEN);
        out.push('\n');
    }
    for m in &result.matches {
        out.push_str(&format!(
            "{}  {}  {}\n",
            m.name,
            m.sym_kind.as_str(),
            m.location
        ));
        if let Some(sig) = &m.signature {
            out.push_str(&format!("    {}\n", capped(sig)));
        }
    }
    if has_signature {
        out.push_str(consts::FENCE_CLOSE);
        out.push('\n');
    }

    // Modules carry no tainted text (a `path` is extractor-computed, not
    // captured source), so no fence is needed here — same reasoning
    // `map_tool.rs`'s module doc comment already gives for its own
    // paths-only output.
    if !result.module_matches.is_empty() {
        out.push_str("## modules\n");
        for m in &result.module_matches {
            out.push_str(&format!(
                "{}  {}  {}\n",
                m.path,
                if m.external { "external" } else { "internal" },
                m.id
            ));
        }
    }

    if result.truncation.truncated {
        if let Some(next) = &result.truncation.next_call {
            out.push_str(&format!("... truncated; see: {next}\n"));
        }
    }

    out
}

fn capped(sig: &TaintedString) -> String {
    sig.render_capped(consts::SIGNATURE_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_repo_path_is_a_tool_error() {
        let err = call(&serde_json::json!({ "needle": "foo" })).unwrap_err();
        assert!(err.contains("repo_path"));
    }

    #[test]
    fn missing_needle_is_a_tool_error() {
        let err = call(&serde_json::json!({ "repo_path": "." })).unwrap_err();
        assert!(err.contains("needle"));
    }

    /// INV-5/ADR-0010: a `SymbolMatch.signature` (repo-derived,
    /// `TaintedString`) reaching the text rendering must appear fenced —
    /// one fence around the whole listing, matching the CLI's
    /// `where_cmd.rs::print_human`, which this function mirrors.
    #[test]
    fn tainted_signature_text_is_fenced_in_the_rendered_output() {
        let result = query::FindResult {
            matches: vec![query::SymbolMatch {
                id: carto_core::graph::sym_id("src/lib.rs", "function", "handle", 1),
                name: "handle".to_string(),
                sym_kind: carto_core::graph::SymKind::Function,
                location: "src/lib.rs:1-3".to_string(),
                signature: Some(TaintedString::new(
                    "pub fn handle(req: Request) -> Response",
                    carto_core::taint::Provenance::Syntactic,
                )),
                provenance: carto_core::taint::Provenance::Syntactic,
            }],
            module_matches: vec![],
            truncation: query::Truncation::none(),
        };

        let text = render_text(&result);
        assert!(text.starts_with(consts::FENCE_OPEN));
        assert!(text.contains(consts::FENCE_CLOSE));
        assert!(text.contains("pub fn handle(req: Request) -> Response"));
        // Exactly one fence pair — around the whole listing, not per row.
        assert_eq!(text.matches(consts::FENCE_OPEN).count(), 1);
        assert_eq!(text.matches(consts::FENCE_CLOSE).count(), 1);
    }

    #[test]
    fn no_signature_means_no_fence() {
        let result = query::FindResult {
            matches: vec![query::SymbolMatch {
                id: carto_core::graph::sym_id("src/lib.rs", "function", "handle", 1),
                name: "handle".to_string(),
                sym_kind: carto_core::graph::SymKind::Function,
                location: "src/lib.rs:1-3".to_string(),
                signature: None,
                provenance: carto_core::taint::Provenance::Syntactic,
            }],
            module_matches: vec![],
            truncation: query::Truncation::none(),
        };

        let text = render_text(&result);
        assert!(!text.contains(consts::FENCE_OPEN));
    }
}
