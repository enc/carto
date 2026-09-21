//! HCL `LangExtractor` (ADR-0025/0026) — not spec §5.2's vocabulary (the
//! design spec's §6.2 infra ingestion assumes `terraform show -json`
//! output; ADR-0025 records why this slice parses `.tf` source
//! directly instead). Native tree-sitter grammar via
//! `carto_grammars::hcl_language()`.
//!
//! Unlike every other extractor here, this one has no `.scm` query
//! files: `tree-sitter-hcl`'s grammar carries no named fields at all
//! (`block`/`attribute`/`string_lit` are pure positional/variadic node
//! lists — verified empirically, `docs.rs` isn't reachable from this
//! sandbox), so a tree-sitter query would have to match by node-kind
//! position anyway, no more declarative than the plain recursive walk
//! below. `symbols`/`imports`/`call_sites` all stay empty this slice —
//! HCL contributes only [`super::extractor::RawLiteral`]s, attached to
//! the `File` node by `resolve.rs` (no symbols to attach to).
//!
//! Slice 1 recognizes exactly one shape (ADR-0026's acceptance test):
//! `resource "aws_cloudwatch_metric_alarm" "X" { metric_name = "...",
//! namespace = "..." }`. An interpolated `metric_name` (`"${local.x}Y"`)
//! is structurally distinguishable from a plain literal at parse time
//! (`expression -> literal_value -> string_lit` vs. `expression ->
//! template_expr -> quoted_template`) and is silently not extracted —
//! INV-8's honesty extended to `RawLiteral` (ADR-0026): no value beats a
//! guessed one.

use super::extractor::{ExtractOut, LangExtractor, RawLiteral};
use crate::lang::Lang;
use tree_sitter::{Node, Parser};

pub struct HclExtractor;

impl LangExtractor for HclExtractor {
    fn lang(&self) -> Lang {
        Lang::Hcl
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["tf", "hcl"]
    }

    fn origin(&self) -> &'static str {
        "lang-hcl@1"
    }

    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        let language = carto_grammars::hcl_language();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("tree-sitter-hcl's language must load into a fresh Parser");

        // A parse failure yields nothing extracted rather than a crash —
        // same honesty principle as every other extractor.
        let Some(tree) = parser.parse(src, None) else {
            return ExtractOut::default();
        };

        let mut literals = Vec::new();
        walk_blocks(tree.root_node(), src, &mut literals);

        ExtractOut {
            literals,
            ..ExtractOut::default()
        }
    }
}

/// Recurses through every node looking for `block` nodes — resource
/// blocks are almost always top-level, but this doesn't assume that
/// (a `dynamic` block or similar nesting still gets found).
fn walk_blocks(node: Node, src: &[u8], out: &mut Vec<RawLiteral>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "block" {
            handle_resource_block(child, src, out);
        }
        walk_blocks(child, src, out);
    }
}

/// If `block` is `resource "aws_cloudwatch_metric_alarm" "X" { ... }`,
/// emits one [`RawLiteral`] for its `metric_name` attribute (qualified
/// by its sibling `namespace` attribute, if present and itself a plain
/// literal). No-op for every other block shape.
fn handle_resource_block(block: Node, src: &[u8], out: &mut Vec<RawLiteral>) {
    let (block_type, labels) = block_type_and_labels(block, src);
    if block_type.as_deref() != Some("resource") {
        return;
    }
    if labels.first().map(String::as_str) != Some("aws_cloudwatch_metric_alarm") {
        return;
    }
    let Some(body) = block
        .children(&mut block.walk())
        .find(|c| c.kind() == "body")
    else {
        return;
    };

    let mut namespace_value: Option<String> = None;
    let mut metric_name: Option<(String, u32)> = None;
    for attr in body
        .children(&mut body.walk())
        .filter(|c| c.kind() == "attribute")
    {
        let Some(name) = attribute_name(attr, src) else {
            continue;
        };
        let Some(expr) = attribute_value_node(attr) else {
            continue;
        };
        match name.as_str() {
            "namespace" => namespace_value = plain_string_value(expr, src),
            "metric_name" => {
                if let Some(v) = plain_string_value(expr, src) {
                    metric_name = Some((v, attr.start_position().row as u32 + 1));
                }
            }
            _ => {}
        }
    }

    if let Some((value, line)) = metric_name {
        out.push(RawLiteral {
            position: "aws_cloudwatch_metric_alarm.metric_name".to_string(),
            value,
            qualifier: namespace_value,
            line,
        });
    }
}

/// `block`'s own type (`resource`, `locals`, …) is its first `identifier`
/// child; every subsequent `string_lit` child before its `body` is a
/// label (`resource`'s two: rtype, then name).
fn block_type_and_labels(block: Node, src: &[u8]) -> (Option<String>, Vec<String>) {
    let mut block_type = None;
    let mut labels = Vec::new();
    for child in block.children(&mut block.walk()) {
        match child.kind() {
            "identifier" if block_type.is_none() => block_type = Some(text(src, child)),
            "string_lit" => labels.push(plain_string_lit_text(child, src)),
            _ => {}
        }
    }
    (block_type, labels)
}

fn attribute_name(attr: Node, src: &[u8]) -> Option<String> {
    attr.children(&mut attr.walk())
        .find(|c| c.kind() == "identifier")
        .map(|n| text(src, n))
}

fn attribute_value_node(attr: Node) -> Option<Node> {
    attr.children(&mut attr.walk())
        .find(|c| c.kind() == "expression")
}

/// `expression -> literal_value -> string_lit` is the *only* shape a
/// plain string literal takes in this grammar — an interpolated string
/// parses to `expression -> template_expr -> quoted_template` instead
/// (verified empirically), so reaching a `string_lit` at all here
/// already means "plain". `None` for anything else (a reference, a
/// number, an interpolation, …) — the honest "not a plain literal" case
/// ADR-0026 documents.
fn plain_string_value(expr: Node, src: &[u8]) -> Option<String> {
    let literal_value = expr
        .children(&mut expr.walk())
        .find(|c| c.kind() == "literal_value")?;
    let string_lit = literal_value
        .children(&mut literal_value.walk())
        .find(|c| c.kind() == "string_lit")?;
    Some(plain_string_lit_text(string_lit, src))
}

/// A `string_lit`'s own text: its `template_literal` child, or empty
/// string for `""` (no `template_literal` child at all).
fn plain_string_lit_text(string_lit: Node, src: &[u8]) -> String {
    string_lit
        .children(&mut string_lit.walk())
        .find(|c| c.kind() == "template_literal")
        .map(|n| text(src, n))
        .unwrap_or_default()
}

fn text(src: &[u8], node: Node) -> String {
    std::str::from_utf8(&src[node.start_byte()..node.end_byte()])
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(src: &str) -> ExtractOut {
        HclExtractor.extract(src.as_bytes(), "alarms.tf")
    }

    #[test]
    fn extracts_metric_name_with_namespace_qualifier() {
        let out = extract(
            r#"
resource "aws_cloudwatch_metric_alarm" "gap" {
  alarm_name          = "gap-alarm"
  namespace           = "SIDCloud/Ingest"
  metric_name         = "SequenceGapsTotal"
  treat_missing_data  = "notBreaching"
}
"#,
        );
        assert_eq!(out.literals.len(), 1);
        let lit = &out.literals[0];
        assert_eq!(lit.position, "aws_cloudwatch_metric_alarm.metric_name");
        assert_eq!(lit.value, "SequenceGapsTotal");
        assert_eq!(lit.qualifier.as_deref(), Some("SIDCloud/Ingest"));
        assert_eq!(lit.line, 5);
    }

    #[test]
    fn interpolated_metric_name_is_not_extracted() {
        let out = extract(
            r#"
resource "aws_cloudwatch_metric_alarm" "gap" {
  namespace   = "SIDCloud/Ingest"
  metric_name = "${local.prefix}Drops"
}
"#,
        );
        assert!(out.literals.is_empty());
    }

    #[test]
    fn non_alarm_resource_is_ignored() {
        let out = extract(
            r#"
resource "aws_s3_bucket" "artifacts" {
  metric_name = "not-a-metric"
}
"#,
        );
        assert!(out.literals.is_empty());
    }

    #[test]
    fn missing_namespace_leaves_qualifier_none() {
        let out = extract(
            r#"
resource "aws_cloudwatch_metric_alarm" "gap" {
  metric_name = "PlainName"
}
"#,
        );
        assert_eq!(out.literals.len(), 1);
        assert_eq!(out.literals[0].qualifier, None);
    }

    #[test]
    fn two_alarms_in_different_namespaces_are_distinct() {
        let out = extract(
            r#"
resource "aws_cloudwatch_metric_alarm" "a" {
  namespace   = "SIDCloud/Ingest"
  metric_name = "Errors"
}
resource "aws_cloudwatch_metric_alarm" "b" {
  namespace   = "SIDCloud/Streamer"
  metric_name = "Errors"
}
"#,
        );
        assert_eq!(out.literals.len(), 2);
        assert_eq!(
            out.literals[0].qualifier.as_deref(),
            Some("SIDCloud/Ingest")
        );
        assert_eq!(
            out.literals[1].qualifier.as_deref(),
            Some("SIDCloud/Streamer")
        );
    }
}
