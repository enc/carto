//! Rust `LangExtractor` (spec §5.2/§5.3). Native tree-sitter grammar via
//! `carto_grammars::rust_language()`. Queries live in
//! `lang/queries/rust/*.scm`, embedded via `include_str!` so they're
//! reviewable independently of this file (spec §5.2).

use super::extractor::{
    ExtractOut, ImportedName, LangExtractor, RawCallSite, RawImport, RawSymbol,
};
use crate::graph::SymKind;
use crate::lang::Lang;
use std::collections::BTreeMap;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Node, Parser, Query, QueryCursor};

const SYMBOLS_QUERY: &str = include_str!("queries/rust/symbols.scm");
const IMPORTS_QUERY: &str = include_str!("queries/rust/imports.scm");
const CALLS_QUERY: &str = include_str!("queries/rust/calls.scm");

pub struct RustExtractor;

impl LangExtractor for RustExtractor {
    fn lang(&self) -> Lang {
        Lang::Rust
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn origin(&self) -> &'static str {
        "lang-rust@1"
    }

    fn relative_import_declares_module(&self) -> bool {
        true
    }

    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        let language = carto_grammars::rust_language();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("tree-sitter-rust's language must load into a fresh Parser");

        // A parse failure (or a file tree-sitter's error-recovery can't
        // make sense of at all) yields nothing extracted rather than a
        // crash — consistent with spec §5.3's honesty principle:
        // extracting nothing is better than guessing from a broken tree.
        let Some(tree) = parser.parse(src, None) else {
            return ExtractOut::default();
        };
        let root = tree.root_node();

        ExtractOut {
            symbols: extract_symbols(root, src),
            imports: extract_imports(root, src),
            call_sites: extract_call_sites(root, src),
            declared_namespace: None,
        }
    }
}

fn extract_symbols(root: Node, src: &[u8]) -> Vec<RawSymbol> {
    let language = carto_grammars::rust_language();
    let query = Query::new(&language, SYMBOLS_QUERY).expect("symbols.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    // Keyed by the item node's byte range: an impl-block method's
    // `function_item` is matched twice — once by the plain
    // `symbol.function` pattern (which doesn't know it's nested inside an
    // `impl_item`) and once by the more specific `symbol.method` pattern.
    // Both matches describe the same underlying node, so this dedupes by
    // range, always preferring the `Method` classification when both are
    // present (whichever match tree-sitter happens to produce first).
    let mut by_range: BTreeMap<(usize, usize), RawSymbol> = BTreeMap::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut item_node = None;
        let mut sym_kind = None;
        let mut impl_type_node = None;

        for cap in m.captures {
            match names[cap.index as usize] {
                "symbol.name" => name_node = Some(cap.node),
                "impl.type" => impl_type_node = Some(cap.node),
                "symbol.function" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Function);
                }
                "symbol.struct" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Struct);
                }
                "symbol.enum" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Enum);
                }
                "symbol.trait" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Trait);
                }
                "symbol.const" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Const);
                }
                "symbol.type" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Type);
                }
                "symbol.method" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Method);
                }
                _ => {}
            }
        }

        let (Some(name_node), Some(item_node), Some(sym_kind)) = (name_node, item_node, sym_kind)
        else {
            continue;
        };

        let name = text(src, name_node);
        let qualified_name = match impl_type_node {
            Some(t) => format!("{}::{}", text(src, t), name),
            None => name.clone(),
        };
        let start_line = item_node.start_position().row as u32 + 1;
        let end_line = item_node.end_position().row as u32 + 1;
        let sig_end = item_node
            .child_by_field_name("body")
            .map(|b| b.start_byte())
            .unwrap_or(item_node.end_byte());
        let signature = text_range(src, item_node.start_byte(), sig_end);
        let is_pub = has_visibility_modifier(item_node);

        let key = (item_node.start_byte(), item_node.end_byte());
        let is_method = matches!(sym_kind, SymKind::Method);
        let already_method = by_range
            .get(&key)
            .is_some_and(|existing| matches!(existing.sym_kind, SymKind::Method));
        if !already_method || is_method {
            by_range.insert(
                key,
                RawSymbol {
                    name,
                    qualified_name,
                    sym_kind,
                    start_line,
                    end_line,
                    signature,
                    is_pub,
                },
            );
        }
    }
    by_range.into_values().collect()
}

/// Whether `item_node` has a `pub`/`pub(crate)`/etc. visibility modifier.
/// Not exposed as a named field in tree-sitter-rust's grammar, so this
/// checks direct children by kind rather than `child_by_field_name`.
fn has_visibility_modifier(item_node: Node) -> bool {
    let mut cursor = item_node.walk();
    item_node
        .children(&mut cursor)
        .any(|c| c.kind() == "visibility_modifier")
}

fn extract_imports(root: Node, src: &[u8]) -> Vec<RawImport> {
    let language = carto_grammars::rust_language();
    let query = Query::new(&language, IMPORTS_QUERY).expect("imports.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            match names[cap.index as usize] {
                "mod.decl" => {
                    // Only `mod foo;` (no body) refers to another file;
                    // an inline `mod foo { .. }` is a namespace, not a
                    // file reference — not extracted this slice.
                    if cap.node.child_by_field_name("body").is_none() {
                        if let Some(name_node) = cap.node.child_by_field_name("name") {
                            out.push(RawImport::Relative {
                                levels_up: 0,
                                module_path: text(src, name_node),
                                imported_names: vec![],
                            });
                        }
                    }
                }
                "use.decl" => {
                    if let Some(arg) = cap.node.child_by_field_name("argument") {
                        if let Some((root_seg, imported_name)) = walk_use_tree(arg, src) {
                            out.push(RawImport::Absolute {
                                root: root_seg,
                                // `use_as_clause` isn't walked (see
                                // `walk_use_tree`'s doc comment), so a
                                // `use` this extractor recognizes never
                                // aliases — bound and declared name are
                                // always the same string.
                                imported_names: vec![ImportedName {
                                    declared_name: imported_name.clone(),
                                    bound_name: imported_name,
                                }],
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Walks a `use` argument tree, returning `(root_segment, imported_name)`
/// — both always present when this returns `Some` at all (a `use`
/// declaration this function recognizes always names exactly one item).
/// Handles plain paths (`use crate::orders::parse_order;`,
/// `use serde;`) at arbitrary depth by following the `scoped_identifier`
/// chain's `path` field to its leftmost leaf. Deliberately does not
/// handle `use_list` (`use a::{b, c};`), `use_wildcard` (`use a::*;`), or
/// `use_as_clause` (`use a::b as c;`) — returns `None`, extracting
/// nothing for that declaration rather than guessing (spec §5.3
/// "deliberately modest"; ADR-0008).
fn walk_use_tree(node: Node, src: &[u8]) -> Option<(String, String)> {
    if node.kind() == "scoped_identifier" {
        let path = node.child_by_field_name("path")?;
        let name = node.child_by_field_name("name")?;
        Some((leftmost_text(path, src), text(src, name)))
    } else if node.kind() == "identifier" || node.kind() == "crate" || node.kind() == "self" {
        let t = text(src, node);
        Some((t.clone(), t))
    } else {
        None
    }
}

fn leftmost_text(node: Node, src: &[u8]) -> String {
    if node.kind() == "scoped_identifier" {
        if let Some(path) = node.child_by_field_name("path") {
            return leftmost_text(path, src);
        }
    }
    text(src, node)
}

fn extract_call_sites(root: Node, src: &[u8]) -> Vec<RawCallSite> {
    let language = carto_grammars::rust_language();
    let query = Query::new(&language, CALLS_QUERY).expect("calls.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if names[cap.index as usize] == "call.name" {
                out.push(RawCallSite {
                    callee_name: text(src, cap.node),
                    line: cap.node.start_position().row as u32 + 1,
                });
            }
        }
    }
    out
}

fn text(src: &[u8], node: Node) -> String {
    text_range(src, node.start_byte(), node.end_byte())
}

fn text_range(src: &[u8], start: usize, end: usize) -> String {
    std::str::from_utf8(&src[start..end])
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(src: &str) -> ExtractOut {
        RustExtractor.extract(src.as_bytes(), "test.rs")
    }

    #[test]
    fn extracts_top_level_function() {
        let out = extract("pub fn parse_order(input: &str) -> u64 {\n    1\n}\n");
        assert_eq!(out.symbols.len(), 1);
        let sym = &out.symbols[0];
        assert_eq!(sym.name, "parse_order");
        assert_eq!(sym.qualified_name, "parse_order");
        assert!(matches!(sym.sym_kind, SymKind::Function));
        assert_eq!(sym.start_line, 1);
        assert_eq!(sym.end_line, 3);
        assert!(sym.is_pub);
        assert!(
            sym.signature
                .contains("pub fn parse_order(input: &str) -> u64")
        );
        assert!(!sym.signature.contains('{'));
    }

    #[test]
    fn private_symbol_is_not_pub() {
        let out = extract("fn validate(input: &str) -> bool {\n    true\n}\n");
        assert!(!out.symbols[0].is_pub);
    }

    #[test]
    fn impl_block_method_is_extracted_exactly_once_as_method_not_also_as_function() {
        // Regression: an impl-block function_item used to match both the
        // generic `symbol.function` pattern and the more specific
        // `symbol.method` pattern, producing two RawSymbols for the same
        // node — which then made every impl-block method spuriously
        // ambiguous for cross-file call resolution (two "pub" candidates
        // with the same name instead of one).
        let out = extract(
            "pub struct Order { pub id: u64 }\n\
             impl Order {\n    pub fn summary(&self) -> String {\n        String::new()\n    }\n}\n",
        );
        let summaries: Vec<&RawSymbol> =
            out.symbols.iter().filter(|s| s.name == "summary").collect();
        assert_eq!(summaries.len(), 1, "expected exactly one `summary` symbol");
        assert!(matches!(summaries[0].sym_kind, SymKind::Method));
    }

    #[test]
    fn extracts_struct_enum_trait_const_type() {
        let out = extract(
            "pub struct Order { pub id: u64 }\n\
             pub enum Status { Open, Closed }\n\
             pub trait Auditable {}\n\
             pub const MAX: u64 = 10;\n\
             pub type Id = u64;\n",
        );
        let kinds: Vec<(&str, SymKind)> = out
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.sym_kind))
            .collect();
        assert!(kinds.contains(&("Order", SymKind::Struct)));
        assert!(kinds.contains(&("Status", SymKind::Enum)));
        assert!(kinds.contains(&("Auditable", SymKind::Trait)));
        assert!(kinds.contains(&("MAX", SymKind::Const)));
        assert!(kinds.contains(&("Id", SymKind::Type)));
    }

    #[test]
    fn extracts_impl_block_method_with_qualified_name() {
        let out = extract(
            "pub struct Order { pub id: u64 }\n\
             impl Order {\n    pub fn summary(&self) -> String {\n        String::new()\n    }\n}\n",
        );
        let method = out
            .symbols
            .iter()
            .find(|s| matches!(s.sym_kind, SymKind::Method))
            .expect("method symbol must be extracted");
        assert_eq!(method.name, "summary");
        assert_eq!(method.qualified_name, "Order::summary");
    }

    #[test]
    fn extracts_mod_declaration_without_body_only() {
        let out = extract("mod orders;\nmod inline { pub fn f() {} }\n");
        let mod_decls: Vec<&str> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Relative { module_path, .. } => Some(module_path.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(mod_decls, vec!["orders"]);
    }

    #[test]
    fn extracts_use_declaration_root_and_imported_name() {
        let out = extract("use crate::orders::parse_order;\nuse serde;\n");
        let uses: Vec<(&str, &str)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Absolute {
                    root,
                    imported_names,
                } => Some((root.as_str(), imported_names[0].bound_name.as_str())),
                _ => None,
            })
            .collect();
        assert!(uses.contains(&("crate", "parse_order")));
        assert!(uses.contains(&("serde", "serde")));
    }

    #[test]
    fn use_list_and_wildcard_are_not_extracted() {
        let out = extract("use std::{fs, io};\nuse std::collections::*;\n");
        assert!(out.imports.is_empty());
    }

    #[test]
    fn extracts_plain_and_method_call_sites_but_not_macros() {
        let out = extract(
            "fn handle() {\n    validate();\n    order.summary();\n    println!(\"hi\");\n}\n",
        );
        let names: Vec<&str> = out
            .call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(names.contains(&"validate"));
        assert!(names.contains(&"summary"));
        assert!(!names.contains(&"println"));
    }

    #[test]
    fn does_not_capture_path_qualified_calls() {
        let out = extract("fn handle() {\n    Order::new();\n}\n");
        assert!(out.call_sites.is_empty());
    }
}
