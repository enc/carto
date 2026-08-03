//! `tools/call` dispatch — one module per tool (spec §7.1's set,
//! restricted to what M1 implements: `index`/`where`/`deps`/`map`/
//! `selfcheck`). Every handler reuses the exact core function the CLI
//! command of the same name uses (`carto_core::query::{find,deps,map}`,
//! `carto_core::indexer::build_and_persist`) — no query logic lives in
//! this crate.

mod deps_tool;
mod index_tool;
mod map_tool;
mod selfcheck_tool;
mod where_tool;

use crate::jsonrpc::{INVALID_PARAMS, RpcError};
use crate::render;
use serde_json::Value;

pub fn call(params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "tools/call missing `name`"))?;
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let arguments = if arguments.is_null() {
        serde_json::json!({})
    } else {
        arguments
    };

    let result = match name {
        "index" => index_tool::call(&arguments),
        "where" => where_tool::call(&arguments),
        "deps" => deps_tool::call(&arguments),
        "map" => map_tool::call(&arguments),
        "selfcheck" => selfcheck_tool::call(&arguments),
        other => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown tool `{other}`"),
            ));
        }
    };

    // Tool-level failures (bad repo path, no index yet, ambiguous symbol
    // name, …) become `isError: true` inside a normal JSON-RPC success
    // response, not a protocol-level RpcError — the `tools/call` request
    // itself was well-formed; only the tool's own operation failed. See
    // `render.rs::error_envelope`'s doc for the same distinction.
    Ok(result.unwrap_or_else(|msg| render::error_envelope(&msg)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    /// Every property name in each tool's hand-written `inputSchema`
    /// (`schema.rs`) must be a parameter the corresponding handler
    /// actually reads (`PARAM_NAMES`) — catches drift between the two
    /// mechanically rather than relying on review alone (this file's own
    /// module doc, and `schema.rs`'s, explain why the drift risk exists
    /// at all: hand-written schemas, not `rmcp`-generated ones).
    #[test]
    fn schema_properties_match_each_handlers_declared_params() {
        let handler_params: &[(&str, &[&str])] = &[
            ("index", index_tool::PARAM_NAMES),
            ("where", where_tool::PARAM_NAMES),
            ("deps", deps_tool::PARAM_NAMES),
            ("map", map_tool::PARAM_NAMES),
            ("selfcheck", selfcheck_tool::PARAM_NAMES),
        ];

        for tool in schema::tool_list() {
            let name = tool["name"].as_str().unwrap();
            let (_, params) = handler_params
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| {
                    panic!("tool `{name}` in schema.rs has no PARAM_NAMES entry in this test")
                });

            let schema_props = tool["inputSchema"]["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("tool `{name}` has no `properties` object"));

            for prop_name in schema_props.keys() {
                assert!(
                    params.contains(&prop_name.as_str()),
                    "tool `{name}`'s schema declares property `{prop_name}` but its handler's PARAM_NAMES doesn't list it — likely drift"
                );
            }
            for param in *params {
                assert!(
                    schema_props.contains_key(*param),
                    "tool `{name}`'s handler reads param `{param}` but schema.rs's inputSchema doesn't declare it"
                );
            }
        }
    }

    #[test]
    fn every_advertised_tool_is_dispatchable() {
        for tool in schema::tool_list() {
            let name = tool["name"].as_str().unwrap().to_string();
            // Calling with empty args either succeeds or fails with a
            // tool-level error — either way, `call` must not itself
            // report "unknown tool" for something schema.rs advertises.
            let result = call(&serde_json::json!({ "name": name, "arguments": {} }));
            assert!(
                result.is_ok(),
                "advertised tool `{name}` was rejected as unknown by dispatch"
            );
        }
    }

    #[test]
    fn unknown_tool_name_is_invalid_params() {
        let err = call(&serde_json::json!({ "name": "nonexistent" })).unwrap_err();
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[test]
    fn missing_name_is_invalid_params() {
        let err = call(&serde_json::json!({})).unwrap_err();
        assert_eq!(err.code, INVALID_PARAMS);
    }
}
