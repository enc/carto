//! Python `LangExtractor` (spec §5.2/§5.3, M1.b.2b). Native tree-sitter
//! grammar via `carto_grammars::python_language()`. Queries live in
//! `lang/queries/python/*.scm`, embedded via `include_str!` so they're
//! reviewable independently of this file (spec §5.2).
//!
//! Rust-specific interpretation of spec §5.3 is recorded in ADR-0008;
//! Python's own mapping (the `RawImport` shapes it produces, the
//! underscore-as-visibility heuristic, wildcard/module-qualified-call
//! handling) is recorded in
//! `docs/adr/0011-python-resolution-policy-mapping.md`.

use super::extractor::{
    ExtractOut, ImportedName, LangExtractor, RawCallSite, RawImport, RawSymbol,
};
use crate::graph::SymKind;
use crate::lang::Lang;
use std::collections::BTreeMap;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Node, Parser, Query, QueryCursor};

const SYMBOLS_QUERY: &str = include_str!("queries/python/symbols.scm");
const IMPORTS_QUERY: &str = include_str!("queries/python/imports.scm");
const CALLS_QUERY: &str = include_str!("queries/python/calls.scm");

pub struct PythonExtractor;

impl LangExtractor for PythonExtractor {
    fn lang(&self) -> Lang {
        Lang::Python
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py"]
    }

    fn origin(&self) -> &'static str {
        "lang-python@1"
    }

    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        let language = carto_grammars::python_language();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("tree-sitter-python's language must load into a fresh Parser");

        // A parse failure yields nothing extracted rather than a crash —
        // same honesty principle as the Rust extractor: extracting
        // nothing beats guessing from a broken tree.
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
    let language = carto_grammars::python_language();
    let query = Query::new(&language, SYMBOLS_QUERY).expect("symbols.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    // Keyed by the item node's byte range, same dedup pattern as
    // rust.rs: a method (decorated or not) is matched once generically
    // (Function) and once more specifically (Method); this keeps
    // whichever match classified it as Method.
    let mut by_range: BTreeMap<(usize, usize), RawSymbol> = BTreeMap::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut item_node = None;
        let mut sym_kind = None;
        let mut class_name_node = None;

        for cap in m.captures {
            match names[cap.index as usize] {
                "symbol.name" => name_node = Some(cap.node),
                "class.name" => class_name_node = Some(cap.node),
                "symbol.function" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Function);
                }
                "symbol.class" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Class);
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

        // A bare (undecorated-pattern) function_definition/
        // class_definition whose immediate parent is a
        // decorated_definition is a spurious extra match: tree-sitter's
        // generic patterns above match it regardless of decoration, but
        // the decorated-specific pattern already captures the *outer*
        // decorated_definition node (a different byte range) as the
        // correct symbol for this same def. Byte-range dedup alone can't
        // catch this — the two ranges genuinely differ — so it's
        // filtered here instead, by checking the node's own parent kind
        // rather than which query pattern produced the capture.
        if matches!(item_node.kind(), "function_definition" | "class_definition")
            && item_node
                .parent()
                .is_some_and(|p| p.kind() == "decorated_definition")
        {
            continue;
        }

        let name = text(src, name_node);
        // Python's own attribute syntax ("Order.summary"), not Rust's
        // "::" — spec §4.3 leaves qualified-name spelling to the
        // extractor.
        let qualified_name = match class_name_node {
            Some(c) => format!("{}.{}", text(src, c), name),
            None => name.clone(),
        };
        let start_line = item_node.start_position().row as u32 + 1;
        let end_line = item_node.end_position().row as u32 + 1;
        let sig_end = body_start_byte(item_node).unwrap_or(item_node.end_byte());
        let signature = text_range(src, item_node.start_byte(), sig_end);
        // No visibility keyword in Python; a leading underscore is the
        // closest real convention for "private by convention" — treated
        // as the is_pub gate for tiers (b)/(c), same role Rust's `pub`
        // plays. Dunder methods (`__init__`) fall under this too (not
        // pub), an accepted simplification since they're conventionally
        // invoked implicitly, not via an explicit bare/attribute call
        // this resolver would ever match anyway. See ADR-0011.
        let is_pub = !name.starts_with('_');

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

/// The byte offset where `item_node`'s body starts — used as the end of
/// its "signature" text. `item_node` is either a bare
/// `function_definition`/`class_definition` (body is a direct field) or
/// a `decorated_definition` wrapping one (body lives on the inner
/// `definition` field instead).
fn body_start_byte(item_node: Node) -> Option<usize> {
    let def_node = if item_node.kind() == "decorated_definition" {
        item_node.child_by_field_name("definition")?
    } else {
        item_node
    };
    def_node.child_by_field_name("body").map(|b| b.start_byte())
}

fn extract_imports(root: Node, src: &[u8]) -> Vec<RawImport> {
    let language = carto_grammars::python_language();
    let query = Query::new(&language, IMPORTS_QUERY).expect("imports.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            match names[cap.index as usize] {
                "import.stmt" => out.extend(extract_import_statement(cap.node, src)),
                "import.from_stmt" => out.extend(extract_import_from_statement(cap.node, src)),
                _ => {}
            }
        }
    }
    out
}

/// `import os` / `import pkg.sub` / `import pkg.sub as ps`. Always
/// absolute — Python has no relative form of plain `import`, only
/// `from`. This binds a whole *module*, not a specific declared symbol
/// — there's no separate "declared name" to distinguish from the local
/// binding the way a symbol import has, so `bound_name`/`declared_name`
/// are the same string here (matches this import's pre-alias-fix
/// behavior exactly; see `ImportedName`'s doc comment).
fn extract_import_statement(node: Node, src: &[u8]) -> Option<RawImport> {
    let name_field = node.child_by_field_name("name")?;
    let (root, bound_name) = if name_field.kind() == "aliased_import" {
        let dotted = name_field.child_by_field_name("name")?;
        let alias = name_field.child_by_field_name("alias")?;
        (first_dotted_segment(dotted, src)?, text(src, alias))
    } else if name_field.kind() == "dotted_name" {
        let first = first_dotted_segment(name_field, src)?;
        (first.clone(), first)
    } else {
        return None;
    };
    Some(RawImport::Absolute {
        root,
        imported_names: vec![ImportedName {
            declared_name: bound_name.clone(),
            bound_name,
        }],
    })
}

/// `from pkg import a, b` (absolute) / `from .pkg import a` (relative,
/// one level) / `from . import pkg` (relative, no module path — each
/// imported name is itself a submodule) / `from x import *` (wildcard,
/// not extracted — no `name:` field to find, so this returns `None`
/// exactly like an unrecognized shape would, the same "extract nothing"
/// honesty as Rust's `use_wildcard` exclusion). Each `name:` field's
/// real declared name and (if aliased, `from pkg import real as
/// alias`) its local bound name are both kept — `declared_name` is
/// what `resolve`'s `pub_by_name`/submodule-file lookups are keyed by;
/// `bound_name` is what a call site in *this* file actually spells.
fn extract_import_from_statement(node: Node, src: &[u8]) -> Option<RawImport> {
    let module_name = node.child_by_field_name("module_name")?;

    let mut cursor = node.walk();
    let imported_names: Vec<ImportedName> = node
        .children_by_field_name("name", &mut cursor)
        .filter_map(|n| {
            if n.kind() == "aliased_import" {
                let declared = n.child_by_field_name("name").map(|d| text(src, d))?;
                let bound = n.child_by_field_name("alias").map(|a| text(src, a))?;
                Some(ImportedName {
                    bound_name: bound,
                    declared_name: declared,
                })
            } else {
                let t = text(src, n);
                Some(ImportedName {
                    bound_name: t.clone(),
                    declared_name: t,
                })
            }
        })
        .collect();
    if imported_names.is_empty() {
        return None;
    }

    if module_name.kind() == "relative_import" {
        let mut inner_cursor = module_name.walk();
        let mut levels_up = 0u32;
        let mut module_path = String::new();
        for child in module_name.children(&mut inner_cursor) {
            match child.kind() {
                "import_prefix" => {
                    // A single dot (`from . import x`) means "the
                    // current package" — zero levels up from the
                    // declaring file's own directory; each additional
                    // dot walks up one more (`from .. import x` = one
                    // level up, the parent package). Hence dot_count - 1,
                    // not dot_count.
                    let dot_count = text(src, child).chars().filter(|c| *c == '.').count() as u32;
                    levels_up = dot_count.saturating_sub(1);
                }
                "dotted_name" => module_path = text(src, child),
                _ => {}
            }
        }
        Some(RawImport::Relative {
            levels_up,
            module_path,
            imported_names,
        })
    } else {
        let root = first_dotted_segment(module_name, src)?;
        Some(RawImport::Absolute {
            root,
            imported_names,
        })
    }
}

/// The first `identifier` child of a `dotted_name` node (e.g. `pkg` in
/// `pkg.sub`) — the leftmost segment, same convention Rust's
/// `leftmost_text` uses for `use` paths.
fn first_dotted_segment(dotted_name: Node, src: &[u8]) -> Option<String> {
    let mut cursor = dotted_name.walk();
    dotted_name
        .children(&mut cursor)
        .find(|c| c.kind() == "identifier")
        .map(|n| text(src, n))
}

fn extract_call_sites(root: Node, src: &[u8]) -> Vec<RawCallSite> {
    let language = carto_grammars::python_language();
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
        PythonExtractor.extract(src.as_bytes(), "test.py")
    }

    #[test]
    fn extracts_top_level_function() {
        let out = extract("def parse_order(input):\n    return 1\n");
        assert_eq!(out.symbols.len(), 1);
        let sym = &out.symbols[0];
        assert_eq!(sym.name, "parse_order");
        assert_eq!(sym.qualified_name, "parse_order");
        assert!(matches!(sym.sym_kind, SymKind::Function));
        assert_eq!(sym.start_line, 1);
        assert_eq!(sym.end_line, 2);
        assert!(sym.is_pub);
        assert!(sym.signature.contains("def parse_order(input)"));
        assert!(!sym.signature.contains("return"));
    }

    #[test]
    fn underscore_prefixed_symbol_is_not_pub() {
        let out = extract("def _validate(input):\n    return True\n");
        assert!(!out.symbols[0].is_pub);
    }

    #[test]
    fn extracts_class_and_method_with_qualified_name() {
        let out = extract("class Order:\n    def summary(self):\n        return self.id\n");
        let class = out
            .symbols
            .iter()
            .find(|s| matches!(s.sym_kind, SymKind::Class))
            .expect("class symbol must be extracted");
        assert_eq!(class.name, "Order");

        let method = out
            .symbols
            .iter()
            .find(|s| matches!(s.sym_kind, SymKind::Method))
            .expect("method symbol must be extracted");
        assert_eq!(method.name, "summary");
        assert_eq!(method.qualified_name, "Order.summary");
    }

    #[test]
    fn method_is_extracted_exactly_once_not_also_as_function() {
        let out = extract("class Order:\n    def summary(self):\n        return self.id\n");
        let summaries: Vec<&RawSymbol> =
            out.symbols.iter().filter(|s| s.name == "summary").collect();
        assert_eq!(summaries.len(), 1, "expected exactly one `summary` symbol");
        assert!(matches!(summaries[0].sym_kind, SymKind::Method));
    }

    #[test]
    fn decorated_method_range_includes_decorator_line_and_is_extracted_once() {
        let out = extract(
            "class Order:\n    @property\n    def summary(self):\n        return self.id\n",
        );
        let summaries: Vec<&RawSymbol> =
            out.symbols.iter().filter(|s| s.name == "summary").collect();
        assert_eq!(summaries.len(), 1, "expected exactly one `summary` symbol");
        assert!(matches!(summaries[0].sym_kind, SymKind::Method));
        assert_eq!(summaries[0].start_line, 2); // the @property line, not the def line
        assert!(summaries[0].signature.contains("@property"));
    }

    #[test]
    fn decorated_top_level_function_is_extracted_as_function_not_method() {
        let out = extract("@some_decorator\ndef standalone():\n    pass\n");
        assert_eq!(out.symbols.len(), 1);
        assert!(matches!(out.symbols[0].sym_kind, SymKind::Function));
        assert_eq!(out.symbols[0].start_line, 1);
    }

    /// Test-only shorthand for an unaliased `ImportedName` — bound and
    /// declared name are the same string, the common case exercised by
    /// most of these tests; the aliased case gets its own assertions
    /// where it matters (`extracts_aliased_from_import`).
    fn name(s: &str) -> ImportedName {
        ImportedName {
            bound_name: s.to_string(),
            declared_name: s.to_string(),
        }
    }

    #[test]
    fn extracts_absolute_import_and_bound_name() {
        let out = extract("import os\nimport pkg.sub as ps\n");
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
        assert!(uses.contains(&("os", "os")));
        assert!(uses.contains(&("pkg", "ps")));
    }

    #[test]
    fn extracts_relative_import_with_multiple_names_and_dot_count() {
        // A single dot means "the current package" -- zero levels up
        // from the declaring file's own directory.
        let out = extract("from .orders import parse_order, validate\n");
        let rel = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Relative {
                    levels_up,
                    module_path,
                    imported_names,
                } => Some((*levels_up, module_path.as_str(), imported_names.clone())),
                _ => None,
            })
            .expect("relative import must be extracted");
        assert_eq!(rel.0, 0);
        assert_eq!(rel.1, "orders");
        assert_eq!(rel.2, vec![name("parse_order"), name("validate")]);
    }

    #[test]
    fn extracts_aliased_from_import_with_distinct_bound_and_declared_names() {
        // `from .orders import parse_order as po` -- the declared name
        // (what `pub_by_name` is keyed by) and the bound name (what a
        // call site in this file actually spells) genuinely differ.
        let out = extract("from .orders import parse_order as po\n");
        let rel = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Relative { imported_names, .. } => Some(imported_names.clone()),
                _ => None,
            })
            .expect("relative import must be extracted");
        assert_eq!(
            rel,
            vec![ImportedName {
                bound_name: "po".to_string(),
                declared_name: "parse_order".to_string(),
            }]
        );
    }

    #[test]
    fn extracts_dotted_relative_import_with_two_levels_up() {
        // Two dots means "the parent package" -- one level up from the
        // declaring file's own directory.
        let out = extract("from ..pkg import thing\n");
        let levels_up = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Relative { levels_up, .. } => Some(*levels_up),
                _ => None,
            })
            .unwrap();
        assert_eq!(levels_up, 1);
    }

    #[test]
    fn extracts_bare_relative_submodule_import() {
        let out = extract("from . import orders\n");
        let rel = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Relative {
                    levels_up,
                    module_path,
                    imported_names,
                } => Some((*levels_up, module_path.clone(), imported_names.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(rel.0, 0);
        assert_eq!(rel.1, "");
        assert_eq!(rel.2, vec![name("orders")]);
    }

    #[test]
    fn wildcard_import_is_not_extracted() {
        let out = extract("from x import *\n");
        assert!(out.imports.is_empty());
    }

    #[test]
    fn extracts_plain_and_attribute_call_sites_but_not_decorators() {
        let out = extract("def handle():\n    validate()\n    order.summary()\n");
        let names: Vec<&str> = out
            .call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(names.contains(&"validate"));
        assert!(names.contains(&"summary"));
    }
}
