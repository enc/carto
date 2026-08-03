//! carto-mcp: the MCP stdio server, mounted by `carto-cli`'s `serve`
//! command (spec §7.3). Hand-rolled newline-delimited JSON-RPC 2.0 rather
//! than `rmcp` — `docs/adr/0018-mcp-transport-hand-rolled-jsonrpc.md`
//! records why. Depends on `carto-core` only: `serde`/`serde_json` for
//! the wire format, nothing else — no async runtime, no HTTP stack,
//! keeping INV-1 trivially true rather than an argument about feature
//! flags.
//!
//! Every tool handler (`tools/`) calls the exact same core function the
//! CLI command of the same name calls (`carto_core::query::{find,deps,
//! map}`, `carto_core::indexer::build_and_persist`) — spec §7.1's
//! "commands = MCP tools, same core functions" is why `carto-core`'s
//! `query` module doc already anticipated this crate before it existed.

mod jsonrpc;
mod render;
mod schema;
mod server;
mod tools;

pub use server::{PROTOCOL_VERSION, serve};
