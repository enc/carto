//! The MCP stdio server loop: reads newline-delimited JSON-RPC requests
//! from `reader`, dispatches, writes newline-delimited responses to
//! `writer`. Synchronous — no tokio, no async runtime (ADR-0018).
//!
//! Handles `initialize`, `notifications/initialized` (and any other
//! `notifications/*`, silently ignored if unrecognized — the JSON-RPC
//! convention), `ping`, `tools/list`, `tools/call`. Any other method is a
//! JSON-RPC `-32601 Method not found` error. A malformed line never kills
//! the loop — see [`jsonrpc::parse_line`]'s error path.

use crate::jsonrpc::{self, METHOD_NOT_FOUND, Request, RpcError};
use crate::{schema, tools};
use carto_core::consts::BIN_NAME;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// The MCP protocol version this server speaks. Fixed rather than
/// negotiated against the client's `initialize` request — this is a
/// minimal hand-rolled server (ADR-0018) supporting exactly one version,
/// not a compatibility matrix.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Runs the server loop until `reader` reaches EOF (stdin closed).
pub fn serve<R: BufRead, W: Write>(mut reader: R, mut writer: W) -> std::io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(()); // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        handle_line(trimmed, &mut writer)?;
    }
}

fn handle_line<W: Write>(line: &str, writer: &mut W) -> std::io::Result<()> {
    match jsonrpc::parse_line(line) {
        Err((id, err)) => write_message(writer, &jsonrpc::error_response(id, &err)),
        Ok(req) => {
            let is_notification = req.id.is_none();
            let outcome = dispatch(&req);
            if is_notification {
                return Ok(()); // notifications never get a response, even on error
            }
            let id = req.id.clone().unwrap_or(Value::Null);
            let message = match outcome {
                Ok(result) => jsonrpc::success_response(id, result),
                Err(err) => jsonrpc::error_response(id, &err),
            };
            write_message(writer, &message)
        }
    }
}

fn write_message<W: Write>(writer: &mut W, value: &Value) -> std::io::Result<()> {
    writeln!(writer, "{value}")?;
    writer.flush()
}

fn dispatch(req: &Request) -> Result<Value, RpcError> {
    match req.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": BIN_NAME, "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": schema::tool_list() })),
        "tools/call" => tools::call(&req.params),
        method if method.starts_with("notifications/") => {
            // Any notification this server doesn't specifically act on
            // (including the expected `notifications/initialized`) is a
            // no-op per JSON-RPC convention — reached only if a
            // misbehaving client sent it *with* an id, since notifications
            // proper never get here (handle_line short-circuits on
            // `is_notification` before calling dispatch's result).
            Ok(Value::Null)
        }
        other => Err(RpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown method `{other}`"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(input: &str) -> String {
        let mut out = Vec::new();
        serve(Cursor::new(input.as_bytes()), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn lines(output: &str) -> Vec<Value> {
        output
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn initialize_handshake_returns_protocol_version_and_capabilities() {
        let out = run(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        let msgs = lines(&out);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(msgs[0]["result"]["capabilities"]["tools"], json!({}));
        assert_eq!(msgs[0]["result"]["serverInfo"]["name"], BIN_NAME);
    }

    #[test]
    fn notification_gets_no_response() {
        let out = run("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
        assert!(out.is_empty());
    }

    #[test]
    fn ping_returns_empty_result() {
        let out = run(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
        let msgs = lines(&out);
        assert_eq!(msgs[0]["result"], json!({}));
    }

    #[test]
    fn tools_list_advertises_every_implemented_tool() {
        let out = run(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let msgs = lines(&out);
        let names: Vec<String> = msgs[0]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        // M1's set (index/where/deps/map/selfcheck) plus `contract`/
        // `orphans` (ADR-0026 — not spec §7.1's tool set).
        assert_eq!(
            names,
            vec![
                "index",
                "where",
                "deps",
                "map",
                "contract",
                "orphans",
                "selfcheck"
            ]
        );
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let out = run(r#"{"jsonrpc":"2.0","id":1,"method":"nonexistent/thing"}"#);
        let msgs = lines(&out);
        assert_eq!(msgs[0]["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn malformed_json_line_does_not_kill_the_loop() {
        // A bad line followed by a good one: both processed, loop
        // survives the first.
        let out = run("not json\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        let msgs = lines(&out);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0]["error"]["code"].as_i64().is_some());
        assert_eq!(msgs[1]["result"], json!({}));
    }

    #[test]
    fn blank_lines_are_skipped() {
        let out = run("\n\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n");
        let msgs = lines(&out);
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn selfcheck_tool_call_round_trips() {
        let out = run(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"selfcheck","arguments":{}}}"#,
        );
        let msgs = lines(&out);
        assert_eq!(msgs[0]["result"]["isError"], false);
        assert!(msgs[0]["result"]["structuredContent"]["carto_version"].is_string());
    }

    #[test]
    fn multiple_requests_in_one_session_each_get_their_own_response() {
        let out = run(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
        );
        let msgs = lines(&out);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["id"], json!(1));
        assert_eq!(msgs[1]["id"], json!(2));
    }
}
