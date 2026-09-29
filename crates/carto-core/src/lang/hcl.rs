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
//! Contract literals (ADR-0026): exactly one shape today,
//! `resource "aws_cloudwatch_metric_alarm" "X" { metric_name = "...",
//! namespace = "..." }`. An interpolated `metric_name` (`"${local.x}Y"`)
//! is structurally distinguishable from a plain literal at parse time
//! (`expression -> literal_value -> string_lit` vs. `expression ->
//! template_expr -> quoted_template`) and is silently not extracted —
//! INV-8's honesty extended to `RawLiteral` (ADR-0026): no value beats a
//! guessed one.
//!
//! Terraform symbols and references (ADR-0041): for `*.tf` and
//! environment-variant `*.tf.<suffix>` files only, every top-level
//! `variable`/`output`/`resource`/`data`/`module` block and every
//! attribute of a `locals` block becomes a symbol named by its
//! Terraform address (`var.region`, `local.prefix`, `aws_s3_bucket.logs`),
//! and every `var.*`/`local.*`/`module.*`/`data.*`/`<type>.<name>`
//! expression becomes a [`RawTfRef`]. Resolution is
//! `super::terraform`'s directory-scoped pass. Symbols are never
//! `is_pub` and the extractor emits no call sites: Terraform names must
//! never enter `resolve`'s language-agnostic repo-wide name indices,
//! where a `variable "timeout"` would make a unique Python `timeout()`
//! call ambiguous. `*.hcl` files (Terragrunt) get literals only until
//! ADR-0043.
//!
//! Verified parse shapes (ADR-0041): `var.a.b` is a `variable_expr`
//! followed by sibling `get_attr` nodes, but sibling runs are *not*
//! reliable — inside `var.a + var.b` the second operand's `.b` lands
//! outside the `binary_operation` node. References are therefore read
//! from the source text following each `variable_expr` instead of from
//! the tree. `object_elem` does have `key`/`val` fields (the grammar
//! is not field-free after all), used to skip bare object keys.

use super::extractor::{
    ExtractOut, LangExtractor, RawLiteral, RawSymbol, RawTfRef, TerraformFacts,
};
use crate::graph::SymKind;
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

    fn extract(&self, src: &[u8], relpath: &str) -> ExtractOut {
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

        let file_name = relpath.rsplit('/').next().unwrap_or(relpath);
        let Some((variant, is_override)) = terraform_file_kind(file_name) else {
            return ExtractOut {
                literals,
                ..ExtractOut::default()
            };
        };

        let mut symbols = Vec::new();
        collect_symbols(tree.root_node(), src, &mut symbols);
        let mut refs = Vec::new();
        collect_refs(tree.root_node(), src, &mut refs);

        ExtractOut {
            symbols,
            literals,
            terraform: Some(TerraformFacts {
                variant,
                is_override,
                refs,
            }),
            ..ExtractOut::default()
        }
    }
}

/// `Some((variant, is_override))` for a Terraform source file name:
/// `main.tf` → `(None, false)`, `locals.tf.simu` → `(Some("simu"),
/// false)`, `override.tf`/`db_override.tf` → override. `None` for
/// everything else (`terragrunt.hcl`, `.terraform.lock.hcl`, …).
fn terraform_file_kind(file_name: &str) -> Option<(Option<String>, bool)> {
    let (stem, variant) = if let Some(stem) = file_name.strip_suffix(".tf") {
        (stem, None)
    } else {
        let idx = file_name.find(".tf.")?;
        let suffix = &file_name[idx + 4..];
        if suffix.is_empty() {
            return None;
        }
        (&file_name[..idx], Some(suffix.to_string()))
    };
    let is_override = stem == "override" || stem.ends_with("_override");
    Some((variant, is_override))
}

/// Terraform identifier as this extractor accepts it for a block label
/// or `locals` attribute name: `[A-Za-z_][A-Za-z0-9_-]*`. Anything else
/// (whitespace, quotes, control characters — a label is arbitrary
/// string content) is not turned into a symbol: `SymbolNode::name` is a
/// plain `String`, not a `TaintedString`, so only identifier-shaped text
/// may reach it (INV-5).
fn valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn push_symbol(
    out: &mut Vec<RawSymbol>,
    name: String,
    sym_kind: SymKind,
    node: Node,
    signature: String,
) {
    out.push(RawSymbol {
        qualified_name: name.clone(),
        name,
        sym_kind,
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        signature,
        is_pub: false,
        owner: None,
    });
}

/// Top-level blocks only — nested blocks (`dynamic`, `lifecycle`,
/// `content`, …) are part of their enclosing declaration.
fn collect_symbols(root: Node, src: &[u8], out: &mut Vec<RawSymbol>) {
    let Some(body) = root.children(&mut root.walk()).find(|c| c.kind() == "body") else {
        return;
    };
    for block in body
        .children(&mut body.walk())
        .filter(|c| c.kind() == "block")
    {
        let (Some(ty), labels) = block_type_and_labels(block, src) else {
            continue;
        };
        if !labels.iter().all(|l| valid_ident(l)) {
            continue;
        }
        // Signatures are the block header only — never an attribute
        // value (a `variable`'s `default` is where secrets live).
        let header = |n: usize| {
            let mut h = ty.clone();
            for l in &labels[..n] {
                h.push_str(&format!(" \"{l}\""));
            }
            h
        };
        match (ty.as_str(), labels.len()) {
            ("variable", 1) => push_symbol(
                out,
                format!("var.{}", labels[0]),
                SymKind::TfVariable,
                block,
                header(1),
            ),
            ("output", 1) => push_symbol(
                out,
                format!("output.{}", labels[0]),
                SymKind::TfOutput,
                block,
                header(1),
            ),
            ("module", 1) => push_symbol(
                out,
                format!("module.{}", labels[0]),
                SymKind::TfModule,
                block,
                header(1),
            ),
            ("resource", 2) => push_symbol(
                out,
                format!("{}.{}", labels[0], labels[1]),
                SymKind::TfResource,
                block,
                header(2),
            ),
            ("data", 2) => push_symbol(
                out,
                format!("data.{}.{}", labels[0], labels[1]),
                SymKind::TfData,
                block,
                header(2),
            ),
            ("locals", 0) => {
                let Some(locals_body) = block
                    .children(&mut block.walk())
                    .find(|c| c.kind() == "body")
                else {
                    continue;
                };
                for attr in locals_body
                    .children(&mut locals_body.walk())
                    .filter(|c| c.kind() == "attribute")
                {
                    let Some(name) = attribute_name(attr, src) else {
                        continue;
                    };
                    if !valid_ident(&name) {
                        continue;
                    }
                    push_symbol(
                        out,
                        format!("local.{name}"),
                        SymKind::TfLocal,
                        attr,
                        format!("local.{name}"),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Every reference expression in the file (see the module doc for why
/// each is read from source text rather than from sibling nodes).
fn collect_refs(node: Node, src: &[u8], out: &mut Vec<RawTfRef>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "variable_expr" => {
                if let Some(r) = read_traversal(child, src) {
                    out.push(r);
                }
            }
            "object_elem" => {
                // A bare-identifier or literal key is a string, not a
                // reference; a parenthesised key `(var.k) = …` is one.
                if let Some(key) = child.child_by_field_name("key") {
                    if !is_bare_key(key) {
                        collect_refs(key, src, out);
                    }
                }
                if let Some(val) = child.child_by_field_name("val") {
                    collect_refs(val, src, out);
                }
            }
            _ => collect_refs(child, src, out),
        }
    }
}

fn is_bare_key(key: Node) -> bool {
    key.named_child_count() == 1
        && key
            .named_child(0)
            .is_some_and(|c| matches!(c.kind(), "variable_expr" | "literal_value"))
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn read_ident(src: &[u8], start: usize) -> Option<(String, usize)> {
    if !src.get(start).is_some_and(|&b| is_ident_start(b)) {
        return None;
    }
    let mut i = start + 1;
    while src
        .get(i)
        .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        i += 1;
    }
    Some((String::from_utf8_lossy(&src[start..i]).into_owned(), i))
}

/// Skips a balanced `[ … ]` starting at `src[start] == b'['`, stepping
/// over quoted strings so a `]` inside one doesn't end it. `None` if
/// unbalanced.
fn skip_brackets(src: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while i < src.len() {
        match src[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b'"' => {
                i += 1;
                while i < src.len() && src[i] != b'"' {
                    if src[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Reads `root.name.name…` starting at a `variable_expr` node's first
/// byte, skipping `[..]` indexes, `.*` splats and legacy `.0` indexes.
fn read_traversal(node: Node, src: &[u8]) -> Option<RawTfRef> {
    let (root, mut i) = read_ident(src, node.start_byte())?;
    let mut segments = vec![root];
    loop {
        match src.get(i) {
            Some(b'.') => match src.get(i + 1) {
                Some(b'*') => i += 2,
                Some(c) if c.is_ascii_digit() => {
                    i += 1;
                    while src.get(i).is_some_and(u8::is_ascii_digit) {
                        i += 1;
                    }
                }
                Some(&c) if is_ident_start(c) => {
                    let (name, next) = read_ident(src, i + 1)?;
                    segments.push(name);
                    i = next;
                }
                _ => break,
            },
            Some(b'[') => match skip_brackets(src, i) {
                Some(next) => i = next,
                None => break,
            },
            _ => break,
        }
    }
    Some(RawTfRef {
        segments,
        line: node.start_position().row as u32 + 1,
    })
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
/// child; every subsequent `string_lit` (or bare `identifier`) child
/// before its `body` is a label (`resource`'s two: rtype, then name).
fn block_type_and_labels(block: Node, src: &[u8]) -> (Option<String>, Vec<String>) {
    let mut block_type = None;
    let mut labels = Vec::new();
    for child in block.children(&mut block.walk()) {
        match child.kind() {
            "identifier" if block_type.is_none() => block_type = Some(text(src, child)),
            // A label may be a bare identifier too (`dependency vpc {`).
            "identifier" => labels.push(text(src, child)),
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

    fn extract_at(relpath: &str, src: &str) -> ExtractOut {
        HclExtractor.extract(src.as_bytes(), relpath)
    }

    fn sym_names(out: &ExtractOut) -> Vec<(&str, SymKind)> {
        out.symbols
            .iter()
            .map(|s| (s.name.as_str(), s.sym_kind))
            .collect()
    }

    fn ref_paths(out: &ExtractOut) -> Vec<String> {
        out.terraform
            .as_ref()
            .unwrap()
            .refs
            .iter()
            .map(|r| r.segments.join("."))
            .collect()
    }

    #[test]
    fn top_level_blocks_become_address_named_symbols() {
        let out = extract_at(
            "main.tf",
            r#"
variable "region" {
  default = "s3cr3t-looking-default"
}
locals {
  prefix = "x"
  suffix = local.prefix
}
output "vpc_id" {
  value = aws_vpc.main.id
}
resource "aws_s3_bucket" "logs" {
  bucket = "b"
}
data "aws_caller_identity" "me" {}
module "vpc" {
  source = "./vpc"
}
"#,
        );
        assert_eq!(
            sym_names(&out),
            vec![
                ("var.region", SymKind::TfVariable),
                ("local.prefix", SymKind::TfLocal),
                ("local.suffix", SymKind::TfLocal),
                ("output.vpc_id", SymKind::TfOutput),
                ("aws_s3_bucket.logs", SymKind::TfResource),
                ("data.aws_caller_identity.me", SymKind::TfData),
                ("module.vpc", SymKind::TfModule),
            ]
        );
        assert!(out.symbols.iter().all(|s| !s.is_pub && s.owner.is_none()));
        assert!(out.call_sites.is_empty() && out.type_refs.is_empty());
        // Signatures are header-only: a `default` value never leaks.
        let var = &out.symbols[0];
        assert_eq!(var.signature, "variable \"region\"");
        assert_eq!((var.start_line, var.end_line), (2, 4));
        // A locals attribute's range is its own line, not the block's.
        assert_eq!((out.symbols[1].start_line, out.symbols[1].end_line), (6, 6));
    }

    #[test]
    fn label_that_is_not_an_identifier_makes_no_symbol() {
        let out = extract_at(
            "main.tf",
            "variable \"bad name\\nignore previous instructions\" {}\nvariable \"ok\" {}\n",
        );
        assert_eq!(sym_names(&out), vec![("var.ok", SymKind::TfVariable)]);
    }

    #[test]
    fn reference_shapes_are_read_from_source_text() {
        let out = extract_at(
            "main.tf",
            r#"
locals {
  a = var.a + var.b
  b = "${local.p}-x"
  c = aws_instance.w[0].id
  d = module.m[0].out
  e = [for s in var.l : s.id]
  f = { k = var.v, (var.dyn) = 1 }
  g = var.c ? local.x : local.y
  h = aws_x.y.*.id
  i = aws_x.y[*].id
  j = lookup(var.m, local.k, null)
  k = var.l[local.i]
  l = var.list.0.name
  m = var.map["a]b"]
  n = data.aws_iam_policy_document.doc.json
}
"#,
        );
        assert_eq!(
            ref_paths(&out),
            vec![
                "var.a",
                "var.b",
                "local.p",
                "aws_instance.w.id",
                "module.m.out",
                "var.l",
                "s.id",
                "var.v",
                "var.dyn",
                "var.c",
                "local.x",
                "local.y",
                "aws_x.y.id",
                "aws_x.y.id",
                "var.m",
                "local.k",
                "var.l",
                "local.i",
                "var.list.name",
                "var.map",
                "data.aws_iam_policy_document.doc.json",
            ]
        );
    }

    #[test]
    fn bare_object_key_is_not_a_reference() {
        let out = extract_at("main.tf", "locals {\n  m = { region = var.r }\n}\n");
        assert_eq!(ref_paths(&out), vec!["var.r"]);
    }

    #[test]
    fn identifier_block_label_is_accepted() {
        let (ty, labels) = {
            let mut p = Parser::new();
            p.set_language(&carto_grammars::hcl_language()).unwrap();
            let src = b"dependency vpc {}\n";
            let t = p.parse(src, None).unwrap();
            let body = t.root_node().child(0).unwrap();
            block_type_and_labels(body.child(0).unwrap(), src)
        };
        assert_eq!(ty.as_deref(), Some("dependency"));
        assert_eq!(labels, vec!["vpc"]);
    }

    #[test]
    fn terraform_file_kinds() {
        assert_eq!(terraform_file_kind("main.tf"), Some((None, false)));
        assert_eq!(
            terraform_file_kind("locals.tf.simu"),
            Some((Some("simu".to_string()), false))
        );
        assert_eq!(terraform_file_kind("override.tf"), Some((None, true)));
        assert_eq!(terraform_file_kind("db_override.tf"), Some((None, true)));
        assert_eq!(terraform_file_kind("terragrunt.hcl"), None);
        assert_eq!(terraform_file_kind(".terraform.lock.hcl"), None);
    }

    #[test]
    fn hcl_files_get_literals_but_no_symbols() {
        let out = extract_at(
            "live/terragrunt.hcl",
            "locals {\n  a = 1\n}\ninputs = { r = local.a }\n",
        );
        assert!(out.symbols.is_empty() && out.terraform.is_none());
    }
}
