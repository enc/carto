//! Go `LangExtractor` (spec §5.2/§5.3 as amended by ADR-0015 — the
//! closing slice of M1.b.2b, the last language in §5.2's v1 set). Native
//! tree-sitter grammar via `carto_grammars::go_language()`. Queries live
//! in `lang/queries/go/*.scm`, embedded via `include_str!` so they're
//! reviewable independently of this file (spec §5.2).
//!
//! Go's own mapping of spec §5.3's language-agnostic resolution policy —
//! `RawImport::PackagePath` (a Go import path names a *directory*, not a
//! file, so none of Rust/Python/PHP's variants fit) and the new
//! directory-scoped resolution tier (a′) this extractor opts into via
//! [`LangExtractor::package_scope_is_directory`] (a Go package's own
//! visibility boundary is the directory, not the file) — is recorded in
//! `docs/adr/0015-go-resolution-policy-mapping.md`.

use super::extractor::{ExtractOut, LangExtractor, RawCallSite, RawImport, RawSymbol, RawTypeRef};
use crate::graph::SymKind;
use crate::lang::Lang;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Node, Parser, Query, QueryCursor};

const SYMBOLS_QUERY: &str = include_str!("queries/go/symbols.scm");
const IMPORTS_QUERY: &str = include_str!("queries/go/imports.scm");
const CALLS_QUERY: &str = include_str!("queries/go/calls.scm");
const TYPES_QUERY: &str = include_str!("queries/go/types.scm");

pub struct GoExtractor;

impl LangExtractor for GoExtractor {
    fn lang(&self) -> Lang {
        Lang::Go
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn origin(&self) -> &'static str {
        "lang-go@1"
    }

    fn package_scope_is_directory(&self) -> bool {
        true
    }

    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        let language = carto_grammars::go_language();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("tree-sitter-go's language must load into a fresh Parser");

        // A parse failure yields nothing extracted rather than a crash —
        // same honesty principle as every other extractor: extracting
        // nothing beats guessing from a broken tree.
        let Some(tree) = parser.parse(src, None) else {
            return ExtractOut::default();
        };
        let root = tree.root_node();

        ExtractOut {
            symbols: extract_symbols(root, src),
            literals: Vec::new(),
            imports: extract_imports(root, src),
            call_sites: extract_call_sites(root, src),
            // Go's selector calls (`pkg.Func()`) are captured, with only
            // the package qualifier discarded (ADR-0015) — the call site
            // itself isn't excluded from resolution, so nothing to
            // count here.
            uncaptured_call_sites: Vec::new(),
            type_refs: extract_type_refs(root, src),
            // Go has no namespace/FQN model (PHP-only, ADR-0012) — package
            // membership is directory-based, handled entirely by
            // `package_scope_is_directory` + `resolve`'s tier (a′), not
            // by an FQN index.
            declared_namespace: None,
        }
    }
}

fn extract_symbols(root: Node, src: &[u8]) -> Vec<RawSymbol> {
    let language = carto_grammars::go_language();
    let query = Query::new(&language, SYMBOLS_QUERY).expect("symbols.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut item_node = None;
        let mut sym_kind = None;

        for cap in m.captures {
            match names[cap.index as usize] {
                "symbol.name" => name_node = Some(cap.node),
                "symbol.function" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Function);
                }
                "symbol.method" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Method);
                }
                "symbol.type_spec" => {
                    item_node = Some(cap.node);
                    // Decided by the spec's own `type:` field, not a
                    // second query pattern per kind — see
                    // `type_spec_kind`'s own doc comment.
                    sym_kind = Some(type_spec_kind(cap.node));
                }
                "symbol.const" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Const);
                }
                "symbol.var" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Var);
                }
                _ => {}
            }
        }

        let (Some(name_node), Some(item_node), Some(sym_kind)) = (name_node, item_node, sym_kind)
        else {
            continue;
        };

        let name = text(src, name_node);
        // ADR-0032: `owner` is the receiver type name, only meaningful
        // for a method — Go has no other symbol kind with an owning
        // type (a `type_spec` is the type itself, not owned by one).
        let owner = match sym_kind {
            SymKind::Method => receiver_type_name(item_node, src),
            _ => None,
        };
        let qualified_name = match &owner {
            // Go's own selector spelling (`Order.Summary`), the same
            // separator Go source itself uses to call a method value —
            // distinct from Rust's/PHP's "::", spec §4.3 leaves
            // qualified-name spelling to the extractor.
            Some(recv) => format!("{recv}.{name}"),
            None => name.clone(),
        };
        let start_line = item_node.start_position().row as u32 + 1;
        let end_line = item_node.end_position().row as u32 + 1;
        let sig_end = item_node
            .child_by_field_name("body")
            .map(|b| b.start_byte())
            .unwrap_or(item_node.end_byte());
        let signature = text_range(src, item_node.start_byte(), sig_end);
        // Go's is_pub is a hard language rule, not a convention proxy the
        // way Python's underscore prefix is: a name's exportedness is
        // its own first-rune case, full stop — see `is_exported`.
        let is_pub = is_exported(&name);

        out.push(RawSymbol {
            name,
            qualified_name,
            sym_kind,
            start_line,
            end_line,
            signature,
            is_pub,
            owner,
        });
    }
    out
}

/// Whether an identifier is exported — Go's actual rule (not a
/// convention proxy the way Python's leading underscore is): the first
/// Unicode scalar of the name is an uppercase letter. `char::is_uppercase`
/// mirrors Go's own `unicode.IsUpper`-based rule closely enough for any
/// real identifier (Go identifiers are letters/digits/underscore, and no
/// digit/underscore start is legal Go syntax to begin with).
fn is_exported(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

/// A `type_spec` node's own `type:` field decides its `SymKind`:
/// `struct_type` -> `Struct`, `interface_type` -> `Interface`, anything
/// else (a named type over `int`, a slice, a map, ...) -> plain `Type`.
/// One capture in symbols.scm, no second query pattern to dedupe against
/// — unlike PHP's class-like-container patterns, which each need their
/// own pattern since PHP's grammar itself has a distinct node kind per
/// container (`class_declaration` vs. `interface_declaration`, ...);
/// Go's grammar has exactly one node kind (`type_spec`) for all of
/// these, distinguished only by its own `type:` field's node kind.
/// `type_alias` (`type X = Y`) never reaches this function — it's
/// always `SymKind::Type` unconditionally (see symbols.scm's own
/// comment for why).
fn type_spec_kind(type_spec: Node) -> SymKind {
    match type_spec.child_by_field_name("type").map(|t| t.kind()) {
        Some("struct_type") => SymKind::Struct,
        Some("interface_type") => SymKind::Interface,
        _ => SymKind::Type,
    }
}

/// A method's receiver type name (`func (o *Order) Summary() {...}` ->
/// `"Order"`), unwrapping a pointer receiver (`*Order`) and a generic
/// receiver (`Order[T]`) down to the underlying `type_identifier`.
/// `None` for a syntactically malformed receiver (shouldn't occur for
/// text that parsed as a `method_declaration` at all, but a raw
/// tree-sitter walk over possibly-partial/error-recovered source is
/// never assumed total) — falls back to the bare method name as its own
/// `qualified_name`, the same "extract something honest rather than
/// crash" tolerance every other extractor has.
fn receiver_type_name(method_decl: Node, src: &[u8]) -> Option<String> {
    let receiver_list = method_decl.child_by_field_name("receiver")?;
    let mut cursor = receiver_list.walk();
    let param = receiver_list
        .children(&mut cursor)
        .find(|c| c.kind() == "parameter_declaration")?;
    let mut ty = param.child_by_field_name("type")?;
    loop {
        match ty.kind() {
            "pointer_type" => ty = ty.named_child(0)?,
            "generic_type" => ty = ty.child_by_field_name("type")?,
            "type_identifier" => return Some(text(src, ty)),
            _ => return None,
        }
    }
}

fn extract_imports(root: Node, src: &[u8]) -> Vec<RawImport> {
    let language = carto_grammars::go_language();
    let query = Query::new(&language, IMPORTS_QUERY).expect("imports.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if names[cap.index as usize] == "import.spec" {
                let Some(path_node) = cap.node.child_by_field_name("path") else {
                    continue;
                };
                // Strips the surrounding `"..."` (interpreted) or
                // `` `...` `` (raw) delimiters — Go import paths never
                // contain escape sequences in practice, so trimming the
                // literal's own quote characters is sufficient without
                // parsing `interpreted_string_literal_content`'s escape
                // grammar.
                let path = text(src, path_node)
                    .trim_matches(|c| c == '"' || c == '`')
                    .to_string();
                if !path.is_empty() {
                    out.push(RawImport::PackagePath { path });
                }
            }
        }
    }
    out
}

fn extract_call_sites(root: Node, src: &[u8]) -> Vec<RawCallSite> {
    let language = carto_grammars::go_language();
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

/// ADR-0029: every identifier `types.scm` found in a type position,
/// walked down to head identifiers by [`collect_type_names`]. See that
/// function and `types.scm`'s own module comment for the position list
/// and the deliberate no-recursion cases that avoid double-counting a
/// node two different global patterns could otherwise both reach.
fn extract_type_refs(root: Node, src: &[u8]) -> Vec<RawTypeRef> {
    let language = carto_grammars::go_language();
    let query = Query::new(&language, TYPES_QUERY).expect("types.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if names[cap.index as usize] == "type.pos" {
                collect_type_names(cap.node, src, &mut out);
            }
        }
    }
    out
}

/// Descends a captured type-position subtree down to the head
/// identifier(s) it names, recursing through every wrapper Go's hidden
/// `_type`/`_simple_type` supertypes have (pointer/slice/array/map/
/// channel/generic) and the two container shapes `types.scm` captures
/// whole (`type_arguments`, an embedding interface's own `type_elem`).
/// A package-qualified type (`pkg.Type`) yields only its type name,
/// the qualifier discarded — the same rightmost-identifier policy
/// `calls.scm`'s own selector capture already applies to
/// `pkg.Func(...)`, so a type reference and a call resolve through
/// identical name-matching semantics. `struct_type`, `interface_type`,
/// and `function_type` are deliberately **not** recursed into at
/// all — see `types.scm`'s own module comment for why (their
/// `field_declaration`/`type_elem` children are always independently,
/// globally captured already; recursing here too would double-count
/// them), and Go's builtin type names (`int`, `string`, `bool`, ...)
/// parse as plain `type_identifier`s with no distinguishing node kind
/// this grammar exposes, so — unlike every other language's dedicated
/// `predefined_type`/`primitive_type` node — they're captured the same
/// as any other bare name rather than filtered out.
fn collect_type_names(node: Node, src: &[u8], out: &mut Vec<RawTypeRef>) {
    match node.kind() {
        "type_identifier" => out.push(RawTypeRef {
            name: text(src, node),
            line: node.start_position().row as u32 + 1,
        }),
        "qualified_type" => {
            if let Some(name) = node.child_by_field_name("name") {
                collect_type_names(name, src, out);
            }
        }
        "generic_type" => {
            if let Some(t) = node.child_by_field_name("type") {
                collect_type_names(t, src, out);
            }
            if let Some(args) = node.child_by_field_name("type_arguments") {
                collect_type_names(args, src, out);
            }
        }
        "slice_type" | "array_type" => {
            if let Some(t) = node.child_by_field_name("element") {
                collect_type_names(t, src, out);
            }
        }
        "map_type" => {
            if let Some(t) = node.child_by_field_name("key") {
                collect_type_names(t, src, out);
            }
            if let Some(t) = node.child_by_field_name("value") {
                collect_type_names(t, src, out);
            }
        }
        "channel_type" => {
            if let Some(t) = node.child_by_field_name("value") {
                collect_type_names(t, src, out);
            }
        }
        // Deliberate no-op: a named multiple-return-value list
        // (`func f() (a int, b string)`) is a `parameter_list` of
        // `parameter_declaration`s, already reached — independently,
        // globally — by `types.scm`'s own unanchored
        // `parameter_declaration` pattern. Recursing here too would
        // double-count every named return type (see `types.scm`'s
        // module comment).
        "parameter_list" => {}
        "pointer_type" | "negated_type" | "parenthesized_type" | "type_elem" | "type_arguments" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_type_names(child, src, out);
            }
        }
        _ => {}
    }
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
        GoExtractor.extract(src.as_bytes(), "test.go")
    }

    fn go(body: &str) -> String {
        format!("package main\n\n{body}")
    }

    fn type_ref_names(out: &ExtractOut) -> Vec<&str> {
        out.type_refs.iter().map(|t| t.name.as_str()).collect()
    }

    // ADR-0029: struct field, embedded-interface-as-field, parameter,
    // and result types — nothing captured any of these before.
    #[test]
    fn extracts_type_refs_from_field_parameter_and_result_positions() {
        let out = extract(&go(
            "type Order struct {\n\tID string\n}\n\ntype Handler struct {\n\tOrder Order\n}\n\nfunc Make() Order {\n\treturn Order{}\n}\n\nfunc Take(o Order) {}\n",
        ));
        let names = type_ref_names(&out);
        assert_eq!(
            names.iter().filter(|&&n| n == "Order").count(),
            4,
            "field + result type + parameter + the composite literal in Make's body, got {names:?}"
        );
    }

    #[test]
    fn named_multiple_return_values_are_captured_exactly_once() {
        // The double-match risk types.scm's own comment calls out:
        // `result:` captures the whole parameter_list, but its
        // parameter_declaration children are already globally
        // captured — must not be counted twice.
        let out = extract(&go(
            "type Order struct{}\n\nfunc Split() (a Order, b Order) {\n\treturn Order{}, Order{}\n}\n",
        ));
        let names = type_ref_names(&out);
        assert_eq!(
            names.iter().filter(|&&n| n == "Order").count(),
            4,
            "two named return types + two composite literals in the body, got {names:?}"
        );
    }

    #[test]
    fn method_receiver_type_is_not_captured_as_a_type_ref() {
        // The receiver's own type is read directly via
        // `child_by_field_name` for symbol qualification (go.rs's own
        // module doc), not through a `parameter_declaration` — Go
        // parses `receiver: (parameter_list (parameter_declaration
        // type: ...))`, so this actually verifies it *is* captured
        // (parameter_declaration's `type:` is unanchored/global) and
        // resolves like any other reference, not that it's excluded.
        let out = extract(&go(
            "type Order struct{}\n\nfunc (o Order) Summary() string {\n\treturn \"\"\n}\n",
        ));
        let names = type_ref_names(&out);
        assert!(names.contains(&"Order"));
    }

    #[test]
    fn pointer_slice_map_and_generic_wrappers_recurse_to_the_element_type() {
        let out = extract(&go(
            "type Order struct{}\n\nfunc Take(a *Order, b []Order, c map[string]Order) {}\n",
        ));
        let names = type_ref_names(&out);
        assert_eq!(names.iter().filter(|&&n| n == "Order").count(), 3);
    }

    #[test]
    fn qualified_package_type_yields_only_the_type_name() {
        let out = extract(&go("import \"io\"\n\nfunc Take(r io.Reader) {}\n"));
        let names = type_ref_names(&out);
        assert_eq!(names, vec!["Reader"]);
    }

    #[test]
    fn var_const_and_composite_literal_types_are_captured() {
        let out = extract(&go(
            "type Order struct{}\n\nvar shared Order\nconst zero Order = Order{}\n\nfunc make() Order {\n\treturn Order{}\n}\n",
        ));
        let names = type_ref_names(&out);
        // var + const + two composite literals (`zero`'s initializer
        // and `make`'s return) + the const/var declarations' own type
        // annotations — every occurrence should resolve to `Order`.
        assert!(names.iter().filter(|&&n| n == "Order").count() >= 4);
    }

    #[test]
    fn type_assertion_and_conversion_are_captured() {
        let out = extract(&go(
            "type Order struct{}\n\nfunc take(x interface{}) {\n\t_ = x.(Order)\n\t_ = []byte(\"s\")\n}\n",
        ));
        let names = type_ref_names(&out);
        assert!(names.contains(&"Order"));
    }

    #[test]
    fn embedded_interface_is_captured_without_double_counting_generic_arguments() {
        // Interface composition (`type Foo interface { Bar }`) must be
        // captured once; a generic type's own argument list
        // (`Container[Order]`) reaches `type_elem` through a different
        // path (`generic_type`'s `type_arguments`) and must not also be
        // matched by the interface-embedding pattern — the exact
        // collision `types.scm`'s own comment documents avoiding.
        let out = extract(&go(
            "type Bar interface{}\n\ntype Foo interface {\n\tBar\n}\n\ntype Order struct{}\ntype Container[T any] struct{ V T }\n\nfunc take(c Container[Order]) {}\n",
        ));
        let names = type_ref_names(&out);
        assert_eq!(names.iter().filter(|&&n| n == "Bar").count(), 1);
        assert_eq!(
            names.iter().filter(|&&n| n == "Order").count(),
            1,
            "{names:?}"
        );
        assert!(names.contains(&"Container"));
    }

    #[test]
    fn extracts_top_level_function() {
        let out = extract(&go("func ParseOrder(input string) int {\n\treturn 1\n}\n"));
        assert_eq!(out.symbols.len(), 1);
        let sym = &out.symbols[0];
        assert_eq!(sym.name, "ParseOrder");
        assert_eq!(sym.qualified_name, "ParseOrder");
        assert!(matches!(sym.sym_kind, SymKind::Function));
        assert!(sym.is_pub);
        assert!(sym.signature.contains("func ParseOrder(input string) int"));
        assert!(!sym.signature.contains("return"));
    }

    #[test]
    fn unexported_function_is_not_pub() {
        let out = extract(&go(
            "func normalize(input string) string {\n\treturn input\n}\n",
        ));
        assert!(!out.symbols[0].is_pub);
    }

    #[test]
    fn extracts_method_with_pointer_receiver_and_qualified_name() {
        let out = extract(&go(
            "type Order struct {\n\tID string\n}\n\nfunc (o *Order) Summary() string {\n\treturn o.ID\n}\n",
        ));
        let method = out
            .symbols
            .iter()
            .find(|s| matches!(s.sym_kind, SymKind::Method))
            .expect("method symbol must be extracted");
        assert_eq!(method.name, "Summary");
        assert_eq!(method.qualified_name, "Order.Summary");
        assert!(method.is_pub);
    }

    #[test]
    fn extracts_method_with_value_receiver() {
        let out = extract(&go(
            "type Order struct {}\n\nfunc (o Order) String() string {\n\treturn \"order\"\n}\n",
        ));
        let method = out
            .symbols
            .iter()
            .find(|s| matches!(s.sym_kind, SymKind::Method))
            .unwrap();
        assert_eq!(method.qualified_name, "Order.String");
    }

    #[test]
    fn struct_and_interface_type_specs_get_the_right_sym_kind() {
        let out = extract(&go(
            "type Order struct {\n\tID string\n}\n\ntype Auditable interface {\n\tAudit()\n}\n\ntype ID int\n",
        ));
        let kind_of = |name: &str| {
            out.symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no symbol named {name}"))
                .sym_kind
        };
        assert!(matches!(kind_of("Order"), SymKind::Struct));
        assert!(matches!(kind_of("Auditable"), SymKind::Interface));
        assert!(matches!(kind_of("ID"), SymKind::Type));
    }

    #[test]
    fn type_alias_is_always_plain_type() {
        let out = extract(&go("type Handler = func(string) error\n"));
        assert!(matches!(out.symbols[0].sym_kind, SymKind::Type));
    }

    #[test]
    fn const_and_var_declarations_emit_one_symbol_per_name() {
        let out = extract(&go(
            "const (\n\tStatusOpen = \"open\"\n\tStatusClosed = \"closed\"\n)\n\nconst Single = 1\n\nvar A, B int\n",
        ));
        let names: Vec<&str> = out.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"StatusOpen"));
        assert!(names.contains(&"StatusClosed"));
        assert!(names.contains(&"Single"));
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));
        assert!(
            out.symbols
                .iter()
                .filter(|s| matches!(s.sym_kind, SymKind::Const))
                .count()
                == 3
        );
        assert!(
            out.symbols
                .iter()
                .filter(|s| matches!(s.sym_kind, SymKind::Var))
                .count()
                == 2
        );
    }

    #[test]
    fn local_const_inside_a_function_is_not_extracted() {
        let out = extract(&go("func run() {\n\tconst limit = 10\n\t_ = limit\n}\n"));
        assert!(
            out.symbols
                .iter()
                .all(|s| !matches!(s.sym_kind, SymKind::Const))
        );
    }

    #[test]
    fn extracts_plain_import() {
        let out = extract(&go("import \"fmt\"\n"));
        let paths: Vec<&str> = out
            .imports
            .iter()
            .map(|i| match i {
                RawImport::PackagePath { path } => path.as_str(),
                _ => panic!("expected PackagePath"),
            })
            .collect();
        assert_eq!(paths, vec!["fmt"]);
    }

    #[test]
    fn extracts_parenthesized_import_group() {
        let out = extract(&go(
            "import (\n\t\"fmt\"\n\t\"github.com/acme/svc/internal/orders\"\n)\n",
        ));
        let paths: Vec<&str> = out
            .imports
            .iter()
            .map(|i| match i {
                RawImport::PackagePath { path } => path.as_str(),
                _ => panic!("expected PackagePath"),
            })
            .collect();
        assert!(paths.contains(&"fmt"));
        assert!(paths.contains(&"github.com/acme/svc/internal/orders"));
    }

    #[test]
    fn extracts_blank_and_aliased_imports_the_same_as_a_plain_one() {
        // `PackagePath` carries no bound-name field, so a blank import
        // (`_`) and an aliased one (`o`) decompose identically to a bare
        // import — see `RawImport::PackagePath`'s own doc comment.
        let out = extract(&go(
            "import (\n\t_ \"github.com/lib/pq\"\n\to \"github.com/acme/svc/internal/orders\"\n)\n",
        ));
        let paths: Vec<&str> = out
            .imports
            .iter()
            .map(|i| match i {
                RawImport::PackagePath { path } => path.as_str(),
                _ => panic!("expected PackagePath"),
            })
            .collect();
        assert!(paths.contains(&"github.com/lib/pq"));
        assert!(paths.contains(&"github.com/acme/svc/internal/orders"));
    }

    #[test]
    fn extracts_plain_and_selector_call_shapes() {
        let out = extract(&go(
            "func handle() {\n\tvalidate()\n\torders.ParseOrder()\n\to.Summary()\n}\n",
        ));
        let names: Vec<&str> = out
            .call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(names.contains(&"validate"));
        assert!(names.contains(&"ParseOrder"));
        assert!(names.contains(&"Summary"));
    }
}
