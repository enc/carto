//! Spec §7.2's output discipline, applied to every tool result: a
//! `structuredContent` block (the same serde type the CLI's `--json`
//! emits, uncapped — the machine payload) plus a compact `content` text
//! block capped at [`carto_core::consts::MCP_TEXT_CAP`] bytes. Structured
//! content is never capped here; each query result already carries its
//! own `Truncation { truncated, next_call }` (spec §7.2's contract,
//! `carto_core::query::Truncation`) for *that* dimension. This module's
//! cap is the second, independent one: the rendered *text* block itself
//! must never exceed 8 KiB, regardless of whether the underlying query
//! result was already truncated.

use carto_core::consts::MCP_TEXT_CAP;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Parses a tool call's `component` param (ADR-0034/0035) — a
/// comma-separated string, the same MCP shape `map_tool.rs`'s own
/// `sections` param already uses for its own repeatable CLI flag.
/// `None`/absent/empty (after trimming each part) means no restriction;
/// [`carto_core::query::QueryGraph::validate_component_filter`] is
/// where an unknown name becomes a tool error, not here — this function
/// only parses the wire shape.
pub fn parse_component_list(args: &Value) -> Option<BTreeSet<String>> {
    let raw = args.get("component").and_then(Value::as_str)?;
    let names: BTreeSet<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if names.is_empty() { None } else { Some(names) }
}

/// Truncates `text` to at most [`MCP_TEXT_CAP`] bytes, on a line boundary
/// so a cut point is never mid-UTF8-sequence (newlines are single-byte
/// ASCII) and never mid-line. Falls back to the nearest character
/// boundary at or before the cap if a single line alone exceeds the cap
/// (no newline to back off to) — still never splits a multi-byte
/// character. Returns the (possibly truncated) text and whether
/// truncation happened.
pub fn cap_text(text: &str) -> (String, bool) {
    if text.len() <= MCP_TEXT_CAP {
        return (text.to_string(), false);
    }
    let bytes = text.as_bytes();
    let mut cut = MCP_TEXT_CAP;
    while cut > 0 && bytes[cut - 1] != b'\n' {
        cut -= 1;
    }
    if cut == 0 {
        cut = MCP_TEXT_CAP;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
    }
    (text[..cut].to_string(), true)
}

/// Builds the standard MCP `tools/call` success envelope: structured JSON
/// content (uncapped) plus a compact text rendering (capped per
/// [`cap_text`], with a trailing note appended if the cap actually cut
/// anything).
pub fn envelope(structured: Value, text: &str) -> Value {
    let (mut capped, was_capped) = cap_text(text);
    if was_capped {
        capped.push_str(&format!(
            "\n... [text response capped at {MCP_TEXT_CAP} bytes; structuredContent carries the full result]"
        ));
    }
    json!({
        "isError": false,
        "structuredContent": structured,
        "content": [{ "type": "text", "text": capped }],
    })
}

/// The envelope for a tool-level failure (bad repo path, no index yet,
/// ambiguous symbol name, …) — distinct from a JSON-RPC protocol error
/// (`jsonrpc.rs::RpcError`, used for malformed requests/unknown
/// tools/methods): the `tools/call` request itself succeeded, the tool's
/// own operation failed. Standard MCP convention: `isError: true` inside
/// a normal JSON-RPC success response, not an RPC-level error.
pub fn error_envelope(message: &str) -> Value {
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": message }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_not_capped() {
        let (out, capped) = cap_text("hello");
        assert_eq!(out, "hello");
        assert!(!capped);
    }

    #[test]
    fn long_text_is_capped_on_a_line_boundary() {
        // Build text well past the cap, made of short lines, so a clean
        // newline boundary exists near the cap.
        let line = "x".repeat(50);
        let text = std::iter::repeat_n(line.as_str(), (MCP_TEXT_CAP / 50) * 3)
            .collect::<Vec<_>>()
            .join("\n");
        let (out, capped) = cap_text(&text);
        assert!(capped);
        assert!(out.len() <= MCP_TEXT_CAP);
        assert!(out.ends_with('\n') || !out.contains('\n') || text.starts_with(&out));
        // Never splits a line: every line in the output is a complete
        // line from the input.
        assert!(text.starts_with(out.trim_end_matches('\n')));
    }

    #[test]
    fn a_single_line_exceeding_the_cap_is_cut_at_a_char_boundary_not_a_newline() {
        // café repeated past the cap, with no newline anywhere — the
        // line-boundary backoff must fall through to char-boundary
        // backoff instead of looping to 0 and producing empty output.
        let text = "café".repeat(MCP_TEXT_CAP);
        let (out, capped) = cap_text(&text);
        assert!(capped);
        assert!(!out.is_empty());
        assert!(out.len() <= MCP_TEXT_CAP);
        // Valid UTF-8 (would have panicked above already via `String`,
        // but assert explicitly that no byte was dropped mid-character).
        assert!(text.starts_with(&out));
    }

    #[test]
    fn envelope_marks_success_and_carries_structured_content() {
        let env = envelope(json!({"a": 1}), "some text");
        assert_eq!(env["isError"], false);
        assert_eq!(env["structuredContent"]["a"], 1);
        assert_eq!(env["content"][0]["text"], "some text");
    }

    #[test]
    fn envelope_appends_a_cap_note_when_truncated() {
        let text = "x".repeat(MCP_TEXT_CAP * 2);
        let env = envelope(json!({}), &text);
        let rendered = env["content"][0]["text"].as_str().unwrap();
        assert!(rendered.contains("capped at"));
    }

    #[test]
    fn error_envelope_marks_failure() {
        let env = error_envelope("no graph.json — run `carto index` first");
        assert_eq!(env["isError"], true);
        assert!(
            env["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("carto index")
        );
    }
}
