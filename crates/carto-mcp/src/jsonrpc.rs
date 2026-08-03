//! Newline-delimited JSON-RPC 2.0 framing for MCP's stdio transport: one
//! JSON object per line, no `Content-Length` header framing (that's the
//! older LSP style, not current MCP). Spec §7.3 names `rmcp` for the MCP
//! server; this crate hand-rolls the transport instead —
//! `docs/adr/0018-mcp-transport-hand-rolled-jsonrpc.md` records why.
//!
//! JSON-RPC's request/notification distinction is the *presence* of the
//! `id` key, not its value — an explicit `"id": null` is still a request
//! awaiting a response (unusual, but spec-legal); a message with no `id`
//! key at all is a notification and gets no response, ever, even on
//! error.

use serde_json::Value;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// A parsed inbound message.
#[derive(Debug)]
pub struct Request {
    /// `None` iff the `id` key was absent from the message (a
    /// notification). `Some(Value::Null)` is a request with an explicit
    /// null id, distinct from a notification.
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
        }
    }
}

/// Parses one line of input into a [`Request`]. On failure, returns the
/// best-effort `id` to report the error against (`Value::Null` when the
/// id itself couldn't be recovered, e.g. the line wasn't even valid JSON)
/// alongside the [`RpcError`] — the caller uses this to still write a
/// well-formed JSON-RPC error response rather than silently dropping a
/// malformed line.
pub fn parse_line(line: &str) -> Result<Request, (Value, RpcError)> {
    let v: Value = serde_json::from_str(line).map_err(|e| {
        (
            Value::Null,
            RpcError::new(PARSE_ERROR, format!("invalid JSON: {e}")),
        )
    })?;
    let obj = v.as_object().ok_or_else(|| {
        (
            Value::Null,
            RpcError::new(INVALID_REQUEST, "request must be a JSON object"),
        )
    })?;
    let id = obj.get("id").cloned();
    let method = match obj.get("method").and_then(Value::as_str) {
        Some(m) => m.to_string(),
        None => {
            return Err((
                id.unwrap_or(Value::Null),
                RpcError::new(INVALID_REQUEST, "missing `method`"),
            ));
        }
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    Ok(Request { id, method, params })
}

/// Builds a successful JSON-RPC response object.
pub fn success_response(id: Value, result: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Builds an error JSON-RPC response object.
pub fn error_response(id: Value, err: &RpcError) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": err.code, "message": err.message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_request_with_params() {
        let req =
            parse_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#).unwrap();
        assert_eq!(req.id, Some(Value::from(1)));
        assert_eq!(req.method, "tools/list");
    }

    #[test]
    fn absent_id_is_a_notification() {
        let req = parse_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert_eq!(req.id, None);
    }

    #[test]
    fn explicit_null_id_is_not_a_notification() {
        let req = parse_line(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#).unwrap();
        assert_eq!(req.id, Some(Value::Null));
    }

    #[test]
    fn malformed_json_reports_parse_error_with_null_id() {
        let (id, err) = parse_line("not json at all").unwrap_err();
        assert_eq!(id, Value::Null);
        assert_eq!(err.code, PARSE_ERROR);
    }

    #[test]
    fn missing_method_reports_invalid_request_but_preserves_id() {
        let (id, err) = parse_line(r#"{"jsonrpc":"2.0","id":5}"#).unwrap_err();
        assert_eq!(id, Value::from(5));
        assert_eq!(err.code, INVALID_REQUEST);
    }

    #[test]
    fn missing_params_defaults_to_null() {
        let req = parse_line(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert_eq!(req.params, Value::Null);
    }
}
