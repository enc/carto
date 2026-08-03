//! MCP tool `index` (spec §7.1's `index`), mirroring
//! `carto-cli/src/index.rs`: same [`carto_core::indexer::build_and_persist`]
//! core call — the whole point of that refactor (see its own module doc)
//! was for this tool and the CLI command to share one write-path
//! implementation.

use crate::render;
use carto_core::indexer;
use carto_core::{pathguard, target};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) const PARAM_NAMES: &[&str] = &["repo_path", "out", "no_gitignore"];

pub fn call(args: &Value) -> Result<Value, String> {
    let repo_path = args
        .get("repo_path")
        .and_then(Value::as_str)
        .ok_or("missing required param `repo_path`")?;
    let out = args.get("out").and_then(Value::as_str).map(PathBuf::from);
    let no_gitignore = args
        .get("no_gitignore")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let t = target::resolve(Path::new(repo_path), &out).map_err(|e| e.to_string())?;

    // Same spec §7.4 "explicit in-repo override" rule as the CLI: only
    // when the caller passed `out` (never the default XDG-cache path)
    // *and* it resolves under the repo root.
    let prospective_out = pathguard::resolve_prospective(&t.out_root).map_err(|e| e.to_string())?;
    let allow_writes_in_repo = out.is_some() && prospective_out.starts_with(&t.repo_root);

    let report = indexer::build_and_persist(
        &t.repo_root,
        &t.out_root,
        !no_gitignore,
        allow_writes_in_repo,
    )
    .map_err(|e| e.to_string())?;

    let text = render_text(&report);
    let structured = serde_json::to_value(&report).map_err(|e| e.to_string())?;
    Ok(render::envelope(structured, &text))
}

fn render_text(report: &indexer::IndexReport) -> String {
    let mut out = format!("indexed {} files\n", report.file_count);
    out.push_str(&format!("  out dir: {}\n", report.out_dir.display()));
    out.push_str(&format!("  nodes:   {}\n", report.node_count));
    out.push_str(&format!("  edges:   {}\n", report.edge_count));
    match &report.commit_sha {
        Some(sha) => out.push_str(&format!("  commit:  {sha}\n")),
        None => out.push_str("  commit:  <unknown>\n"),
    }
    if report.redaction_count > 0 {
        out.push_str(&format!("  redactions: {}\n", report.redaction_count));
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
