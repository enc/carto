//! Hand-written JSON Schemas for `tools/list`, one per tool, mirroring
//! each CLI command's `clap::Args` (`crates/carto-cli/src/{index,
//! where_cmd,deps_cmd,map_cmd}.rs`, `carto-cli/src/selfcheck.rs`).
//!
//! Hand-written rather than derived (`rmcp` would generate these from the
//! same serde types via `schemars` — one of the things ADR-0018 gives up
//! by not depending on it) — a real maintenance cost: a param this file
//! doesn't list, a handler silently ignores. `tools/schema_drift` tests
//! (`tools/mod.rs`) pin every property name here against what each
//! handler actually reads, to catch drift mechanically rather than
//! relying on review alone.
//!
//! Only the tools M1's milestone actually implements are advertised —
//! spec §7.1 also names `impact`, `infra_of`, `code_of`,
//! `unused_permissions`, `ingest`, `plan --semantic`, all M2–M4 work;
//! listing them here would be a capability lie.

use serde_json::{Value, json};

pub fn tool_list() -> Vec<Value> {
    vec![
        json!({
            "name": "index",
            "description": "Build the deterministic structural graph of a repository (symbols, files, modules, imports, calls-best-effort) and persist it to an out-dir. Run this once per repo+out-dir, or after a large change, before where/deps/map — a repo already indexed into a given out-dir keeps that graph available for every later call; re-indexing it again before every question in the same session repeats a real cost for no new information.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Path to the repo to index." },
                    "out": { "type": "string", "description": "Output directory. Defaults to ${XDG_CACHE_HOME:-~/.cache}/carto/<repo-hash>/." },
                    "no_gitignore": { "type": "boolean", "description": "Disable respecting .gitignore. The built-in denylist and .cartoignore still apply regardless.", "default": false },
                },
                "required": ["repo_path"],
            },
        }),
        json!({
            "name": "where",
            "description": "Find symbols by name (substring match by default). Returns id, kind, file:line, and signature for each match.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Repo previously indexed with the `index` tool." },
                    "needle": { "type": "string", "description": "Symbol name to search for." },
                    "out": { "type": "string", "description": "Output directory carto previously indexed into. Defaults to the same location `index` uses by default." },
                    "exact": { "type": "boolean", "description": "Exact, case-sensitive match instead of case-insensitive substring.", "default": false },
                    "limit": { "type": "integer", "description": "Max matches to return.", "default": 50 },
                    "subpath": { "type": "string", "description": "Restrict matches to symbols under this repo-relative directory." },
                },
                "required": ["repo_path", "needle"],
            },
        }),
        json!({
            "name": "deps",
            "description": "Adjacency listing with confidence: what a symbol/file calls or imports (--dir out), or what calls/imports it (--dir in). Every edge carries a confidence (certain/inferred); inferred means verify before acting on it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Repo previously indexed with the `index` tool." },
                    "target": { "type": "string", "description": "Node ID or exact symbol name to start from." },
                    "out": { "type": "string", "description": "Output directory carto previously indexed into." },
                    "dir": { "type": "string", "enum": ["in", "out", "both"], "description": "Which direction to follow edges.", "default": "out" },
                    "depth": { "type": "integer", "description": "How many hops to traverse (1..=5).", "default": 1 },
                    "kinds": { "type": "string", "description": "Comma-separated edge kinds to include (e.g. contains,imports,calls). Defaults to every kind." },
                    "subpath": { "type": "string", "description": "Restrict reported rows to nodes under this repo-relative directory." },
                },
                "required": ["repo_path", "target"],
            },
        }),
        json!({
            "name": "map",
            "description": "Layered overview of a repo: top modules by import fan-in/out, structural entry points, infra/join summary (placeholders until M2/M3). Hard-capped at --budget lines.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo_path": { "type": "string", "description": "Repo previously indexed with the `index` tool." },
                    "out": { "type": "string", "description": "Output directory carto previously indexed into." },
                    "budget": { "type": "integer", "description": "Max lines in the rendered overview.", "default": 200 },
                    "subpath": { "type": "string", "description": "Restrict the overview to this repo-relative directory." },
                    "sections": { "type": "string", "description": "Comma-separated sections to render (counts,modules,entry-points,infra). Defaults to every section. The structured `counts` field is always exact regardless of this filter." },
                },
                "required": ["repo_path"],
            },
        }),
        json!({
            "name": "selfcheck",
            "description": "Environment report: carto version, schema version, grammar mode, confinement status. Useful for confirming the server is the version you expect.",
            "inputSchema": {
                "type": "object",
                "properties": {},
            },
        }),
    ]
}
