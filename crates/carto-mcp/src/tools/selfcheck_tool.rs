//! MCP tool `selfcheck` (spec §7.1: "environment report: versions,
//! grammar mode, landlock status").
//!
//! Deliberately **not** a call into `carto-cli`'s `selfcheck.rs` —
//! that file is the one place INV-2's `std::process::Command` ban is
//! documented as exempted (`clippy.toml`'s comment names that exact
//! path), and moving/duplicating that logic here would blur which file
//! the exemption actually covers. This report is a small, independent
//! set of facts built from the same `carto_core` primitives
//! (`consts`, `pathguard::denylist_digest`) rather than sharing the CLI's
//! richer struct — it doesn't include `git_sha`/`target_triple`
//! (populated via `carto-cli`'s own `build.rs`, scoped to that crate's
//! compilation, not available here) or `min_rust_version` beyond what
//! `carto-mcp`'s own `Cargo.toml` declares (identical value in practice,
//! since the whole workspace shares `rust-version` — spec §14's "verify
//! at implementation time" territory if that ever changes).

use crate::render;
use carto_core::consts::{BIN_NAME, SCHEMA_VERSION};
use carto_core::pathguard;
use serde::Serialize;
use serde_json::Value;

#[cfg(test)]
pub(crate) const PARAM_NAMES: &[&str] = &[];

#[derive(Serialize)]
struct McpSelfcheckReport {
    server_name: &'static str,
    carto_version: &'static str,
    schema_version: u32,
    protocol_version: &'static str,
    pathguard_denylist_digest: String,
}

pub fn call(_args: &Value) -> Result<Value, String> {
    let report = McpSelfcheckReport {
        server_name: BIN_NAME,
        carto_version: env!("CARGO_PKG_VERSION"),
        schema_version: SCHEMA_VERSION,
        protocol_version: crate::server::PROTOCOL_VERSION,
        pathguard_denylist_digest: pathguard::denylist_digest(),
    };

    let text = format!(
        "{} {} (mcp)\n  schema_version:    {}\n  protocol_version:  {}\n  denylist digest:   {}\n",
        report.server_name,
        report.carto_version,
        report.schema_version,
        report.protocol_version,
        report.pathguard_denylist_digest,
    );
    let structured = serde_json::to_value(&report).map_err(|e| e.to_string())?;
    Ok(render::envelope(structured, &text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_a_well_formed_success_envelope() {
        let env = call(&serde_json::json!({})).unwrap();
        assert_eq!(env["isError"], false);
        assert!(env["structuredContent"]["carto_version"].is_string());
    }
}
