//! PHP `LangExtractor` (spec §5.2/§5.3 as amended by ADR-0012). Native
//! tree-sitter grammar via `carto_grammars::php_language()`. Queries live
//! in `lang/queries/php/*.scm`, embedded via `include_str!` so they're
//! reviewable independently of this file (spec §5.2).
//!
//! PHP-specific interpretation of spec §5.3 — the repo-wide FQN index
//! that resolves `use` imports (PHP has no `mod`-style path arithmetic
//! and no walked-repo directory structure to resolve against, unlike
//! Rust and Python), and the deliberate divergence from ADR-0008's
//! Rust precedent of capturing static (`Foo::bar()`) calls — is
//! recorded in `docs/adr/0012-php-resolution-policy-mapping.md`.

use super::extractor::{ExtractOut, LangExtractor, RawCallSite, RawImport, RawSymbol, RawTypeRef};
use crate::graph::SymKind;
use crate::lang::Lang;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Node, Parser, Query, QueryCursor};

const SYMBOLS_QUERY: &str = include_str!("queries/php/symbols.scm");
const IMPORTS_QUERY: &str = include_str!("queries/php/imports.scm");
const CALLS_QUERY: &str = include_str!("queries/php/calls.scm");
const TYPES_QUERY: &str = include_str!("queries/php/types.scm");

pub struct PhpExtractor;

impl LangExtractor for PhpExtractor {
    fn lang(&self) -> Lang {
        Lang::Php
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["php"]
    }

    fn origin(&self) -> &'static str {
        "lang-php@1"
    }

    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        let language = carto_grammars::php_language();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("tree-sitter-php's language must load into a fresh Parser");

        // A parse failure yields nothing extracted rather than a crash —
        // same honesty principle as the Rust and Python extractors:
        // extracting nothing beats guessing from a broken tree.
        let Some(tree) = parser.parse(src, None) else {
            return ExtractOut::default();
        };
        let root = tree.root_node();

        ExtractOut {
            symbols: extract_symbols(root, src),
            literals: Vec::new(),
            imports: extract_imports(root, src),
            call_sites: extract_call_sites(root, src),
            // PHP's path-qualified calls are captured, not excluded
            // (ADR-0012) — nothing to count here.
            uncaptured_call_sites: Vec::new(),
            type_refs: extract_type_refs(root, src),
            declared_namespace: extract_namespace(root, src),
            terraform: None,
        }
    }
}

fn extract_symbols(root: Node, src: &[u8]) -> Vec<RawSymbol> {
    let language = carto_grammars::php_language();
    let query = Query::new(&language, SYMBOLS_QUERY).expect("symbols.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    // Unlike Rust/Python, no byte-range dedup is needed here: PHP's
    // `method_declaration` is a distinct node kind from
    // `function_definition` and only ever appears nested in a
    // class/interface/trait/enum body, so each declaration node is
    // matched by exactly one pattern in symbols.scm (see ADR-0012).
    let mut out = Vec::new();
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
                "symbol.interface" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Interface);
                }
                "symbol.trait" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Trait);
                }
                "symbol.enum" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Enum);
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
        // ADR-0032: `owner` reuses `class_name_node` — already computed
        // for `qualified_name` below, and exactly "the class this
        // symbol is declared in".
        let owner = class_name_node.map(|c| text(src, c));
        // PHP's own separator ("Order::summary"), not Rust's "::"-as-
        // impl-path or Python's "." — coincidentally same spelling as
        // Rust's, since it's also PHP's own scope-resolution operator.
        // Spec §4.3 leaves qualified-name spelling to the extractor.
        let qualified_name = match &owner {
            Some(c) => format!("{c}::{name}"),
            None => name.clone(),
        };
        let start_line = item_node.start_position().row as u32 + 1;
        let end_line = item_node.end_position().row as u32 + 1;
        let sig_end = item_node
            .child_by_field_name("body")
            .map(|b| b.start_byte())
            .unwrap_or(item_node.end_byte());
        let signature = text_range(src, item_node.start_byte(), sig_end);
        let is_pub = if matches!(sym_kind, SymKind::Method) {
            method_is_pub(item_node, src)
        } else {
            // PHP has no top-level visibility keyword; every top-level
            // function/class/interface/trait/enum is exported the same
            // way Python's non-underscore-prefixed names are — except
            // PHP has real visibility keywords for methods, so this
            // extractor doesn't need Python's underscore-as-proxy
            // heuristic there (see below).
            true
        };

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

/// Whether a `method_declaration` is exported for spec §5.3 rule 2 tiers
/// (b)/(c). Unlike Python (no visibility keyword; underscore prefix is a
/// convention proxy), PHP has real `public`/`protected`/`private`
/// keywords — a bare `visibility_modifier` child node whose own text is
/// the keyword itself; a method with none is implicitly `public` (PHP's
/// own default).
fn method_is_pub(item_node: Node, src: &[u8]) -> bool {
    let mut cursor = item_node.walk();
    let modifier = item_node
        .children(&mut cursor)
        .find(|c| c.kind() == "visibility_modifier");
    match modifier {
        None => true,
        Some(m) => {
            let kw = text(src, m);
            kw != "private" && kw != "protected"
        }
    }
}

fn extract_namespace(root: Node, src: &[u8]) -> Option<String> {
    let language = carto_grammars::php_language();
    let query = Query::new(&language, IMPORTS_QUERY).expect("imports.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if names[cap.index as usize] == "namespace.def" {
                // The bracketed anonymous form (`namespace { .. }`, PHP's
                // spelling of "explicitly the global namespace") has no
                // `name:` field — treated the same as no declaration at
                // all (`None`), since both mean "unqualified names live
                // in the global namespace" to `resolve`'s FQN index.
                // Only the first `namespace` statement in the file is
                // used: bracketed multi-namespace-per-file syntax (legacy,
                // rare) isn't modeled — the FQN index assumes one
                // namespace per file, spec §5.3 "deliberately modest".
                if let Some(name_node) = cap.node.child_by_field_name("name") {
                    return Some(text(src, name_node));
                }
                return None;
            }
        }
    }
    None
}

fn extract_imports(root: Node, src: &[u8]) -> Vec<RawImport> {
    let language = carto_grammars::php_language();
    let query = Query::new(&language, IMPORTS_QUERY).expect("imports.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if names[cap.index as usize] == "use.decl" {
                out.extend(extract_use_declaration(cap.node, src));
            }
        }
    }
    out
}

/// Decomposes one `namespace_use_declaration` into a `RawImport::Qualified`
/// per imported name. Handles the plain form (`use App\Orders\Order;`),
/// the aliased form (`use ... as X;`), the `function`/`const` form (`use
/// function App\Orders\parseOrder;` — the leading keyword is ignored:
/// the FQN index doesn't distinguish import kinds), and the grouped form
/// (`use App\Orders\{Order, Handler as H};` — the leading `namespace_name`
/// child is the shared prefix, joined onto each clause inside `body`).
/// A trailing `type`/`const` clause-level keyword is likewise ignored for
/// the same reason.
fn extract_use_declaration(node: Node, src: &[u8]) -> Vec<RawImport> {
    let mut top_cursor = node.walk();
    // Present only for the grouped form — the shared prefix before `{`.
    let group_prefix: Option<String> = node
        .children(&mut top_cursor)
        .find(|c| c.kind() == "namespace_name")
        .map(|n| text(src, n));

    let mut clause_cursor = node.walk();
    let clauses: Vec<Node> = if let Some(body) = node.child_by_field_name("body") {
        let mut body_cursor = body.walk();
        body.children(&mut body_cursor)
            .filter(|c| c.kind() == "namespace_use_clause")
            .collect()
    } else {
        node.children(&mut clause_cursor)
            .filter(|c| c.kind() == "namespace_use_clause")
            .collect()
    };

    clauses
        .into_iter()
        .filter_map(|clause| {
            let mut cc = clause.walk();
            let name_child = clause
                .children(&mut cc)
                .find(|c| c.kind() == "name" || c.kind() == "qualified_name")?;
            let raw = text(src, name_child);
            let fqn = match &group_prefix {
                Some(prefix) => format!("{prefix}\\{raw}"),
                None => raw.clone(),
            };
            let bound_name = clause
                .child_by_field_name("alias")
                .map(|a| text(src, a))
                .unwrap_or_else(|| last_segment(&raw));
            Some(RawImport::Qualified { fqn, bound_name })
        })
        .collect()
}

/// The last `\`-separated segment of a (possibly unqualified) name —
/// the bare identifier tier (b) call resolution matches against.
fn last_segment(name: &str) -> String {
    name.rsplit('\\').next().unwrap_or(name).to_string()
}

fn extract_call_sites(root: Node, src: &[u8]) -> Vec<RawCallSite> {
    let language = carto_grammars::php_language();
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
/// — including `object_creation_expression` and trait `use_declaration`,
/// two shapes nothing else in this extractor produces any edge for at
/// all today.
fn extract_type_refs(root: Node, src: &[u8]) -> Vec<RawTypeRef> {
    let language = carto_grammars::php_language();
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
/// identifier(s) it names, recursing through PHP's `named_type`/
/// `optional_type`/`union_type`/`intersection_type` wrappers and the
/// container shapes `types.scm` captures whole (`base_clause`,
/// `class_interface_clause`, `use_declaration`'s own `use_list` for a
/// multi-trait `use A, B;`). A qualified/relative name (`App\Orders\
/// Order`, `namespace\Order`) yields only its *rightmost* segment via
/// its own `name` child — the same policy every other extractor's
/// qualified-reference capture already applies. `primitive_type`
/// (`string`, `int`, `bool`, `array`, ...) has no symbol to reference
/// and is silently skipped, like every other language's builtin-type
/// exclusion; PHP 8.2's disjunctive-normal-form type (`(A&B)|C`) is a
/// rare enough shape that descending into its own nested unions isn't
/// captured this slice — documented, not silently dropped.
fn collect_type_names(node: Node, src: &[u8], out: &mut Vec<RawTypeRef>) {
    match node.kind() {
        "name" => out.push(RawTypeRef {
            name: text(src, node),
            line: node.start_position().row as u32 + 1,
        }),
        "qualified_name"
        | "relative_name"
        | "named_type"
        | "optional_type"
        | "union_type"
        | "intersection_type"
        | "type_list"
        | "use_list"
        | "base_clause"
        | "class_interface_clause"
        | "use_declaration" => {
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
        PhpExtractor.extract(src.as_bytes(), "test.php")
    }

    fn php(body: &str) -> String {
        format!("<?php\n{body}")
    }

    fn type_ref_names(out: &ExtractOut) -> Vec<&str> {
        out.type_refs.iter().map(|t| t.name.as_str()).collect()
    }

    // ADR-0029: property/parameter/return-type positions — nothing in
    // this extractor captured any of these before.
    #[test]
    fn extracts_type_refs_from_property_parameter_and_return_positions() {
        let out = extract(&php(
            "class Order {}\n\nclass Handler {\n    public Order $order;\n\n    public function take(Order $o): Order {\n        return $o;\n    }\n}\n",
        ));
        let names = type_ref_names(&out);
        assert_eq!(
            names.iter().filter(|&&n| n == "Order").count(),
            3,
            "property + parameter + return type, got {names:?}"
        );
    }

    #[test]
    fn constructor_promoted_property_type_is_captured() {
        let out = extract(&php(
            "class Order {}\n\nclass Handler {\n    public function __construct(public Order $order) {}\n}\n",
        ));
        assert!(type_ref_names(&out).contains(&"Order"));
    }

    #[test]
    fn extends_and_implements_are_both_captured() {
        let out = extract(&php(
            "interface Comparable {}\nclass Base {}\n\nclass Order extends Base implements Comparable {}\n",
        ));
        let names = type_ref_names(&out);
        assert!(names.contains(&"Base"));
        assert!(names.contains(&"Comparable"));
    }

    #[test]
    fn new_foo_is_captured_where_calls_scm_never_looked() {
        // Unlike every call shape calls.scm handles, object creation
        // is not one of them for PHP -- verified against calls.scm
        // itself, not assumed.
        let out = extract(&php(
            "class Order {}\n\nfunction make() {\n    return new Order();\n}\n",
        ));
        assert!(out.call_sites.iter().all(|c| c.callee_name != "Order"));
        assert!(type_ref_names(&out).contains(&"Order"));
    }

    #[test]
    fn qualified_and_relative_object_creation_yield_only_the_final_segment() {
        let out = extract(&php(
            "function make() {\n    return new App\\Orders\\Order();\n}\n",
        ));
        assert_eq!(type_ref_names(&out), vec!["Order"]);
    }

    #[test]
    fn trait_use_is_captured_a_shape_imports_scm_never_looked_at_either() {
        let out = extract(&php(
            "trait Auditable {}\n\nclass Order {\n    use Auditable;\n}\n",
        ));
        assert!(type_ref_names(&out).contains(&"Auditable"));
        assert!(
            out.imports.is_empty(),
            "trait use_declaration is a distinct node kind from namespace use, never an import"
        );
    }

    #[test]
    fn catch_clause_and_union_type_are_captured() {
        let out = extract(&php(
            "class OrderException {}\nclass ValidationException {}\n\nfunction take(int|null $x) {}\n\nfunction run() {\n    try {\n    } catch (OrderException|ValidationException $e) {\n    }\n}\n",
        ));
        let names = type_ref_names(&out);
        assert!(names.contains(&"OrderException"));
        assert!(names.contains(&"ValidationException"));
        assert!(!names.contains(&"int"), "{names:?}"); // primitive_type, skip
    }

    #[test]
    fn primitive_types_are_not_captured() {
        let out = extract(&php(
            "function take(string $a, int $b, ?bool $c): void {}\n",
        ));
        assert!(type_ref_names(&out).is_empty());
    }

    #[test]
    fn extracts_top_level_function() {
        let out = extract(&php("function parse_order($input) {\n    return 1;\n}\n"));
        assert_eq!(out.symbols.len(), 1);
        let sym = &out.symbols[0];
        assert_eq!(sym.name, "parse_order");
        assert_eq!(sym.qualified_name, "parse_order");
        assert!(matches!(sym.sym_kind, SymKind::Function));
        assert!(sym.is_pub);
        assert!(sym.signature.contains("function parse_order($input)"));
        assert!(!sym.signature.contains("return"));
    }

    #[test]
    fn extracts_class_and_public_method_with_qualified_name() {
        let out = extract(&php(
            "class Order {\n    public function summary() {\n        return $this->id;\n    }\n}\n",
        ));
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
        assert_eq!(method.qualified_name, "Order::summary");
        assert!(method.is_pub);
    }

    #[test]
    fn private_and_protected_methods_are_not_pub_but_public_and_bare_are() {
        let out = extract(&php("class Order {\n\
             \x20   private function refresh() {}\n\
             \x20   protected function helper() {}\n\
             \x20   public function summary() {}\n\
             \x20   function bare() {}\n\
             }\n"));
        let is_pub = |name: &str| {
            out.symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no symbol named {name}"))
                .is_pub
        };
        assert!(!is_pub("refresh"));
        assert!(!is_pub("helper"));
        assert!(is_pub("summary"));
        assert!(is_pub("bare"), "no visibility keyword defaults to public");
    }

    #[test]
    fn extracts_interface_trait_and_enum() {
        let out = extract(&php(
            "interface Auditable {\n    public function audit();\n}\n\
             trait Loggable {\n    public function log() {}\n}\n\
             enum Status {\n    case Open;\n    case Closed;\n    public function label() {}\n}\n",
        ));
        let kinds: Vec<(&str, SymKind)> = out
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.sym_kind))
            .collect();
        assert!(kinds.contains(&("Auditable", SymKind::Interface)));
        assert!(kinds.contains(&("Loggable", SymKind::Trait)));
        assert!(kinds.contains(&("Status", SymKind::Enum)));
        assert!(kinds.contains(&("audit", SymKind::Method)));
        assert!(kinds.contains(&("log", SymKind::Method)));
        assert!(kinds.contains(&("label", SymKind::Method)));
    }

    #[test]
    fn method_is_extracted_exactly_once_not_also_as_function() {
        let out = extract(&php(
            "class Order {\n    public function summary() {\n        return 1;\n    }\n}\n",
        ));
        let summaries: Vec<&RawSymbol> =
            out.symbols.iter().filter(|s| s.name == "summary").collect();
        assert_eq!(summaries.len(), 1, "expected exactly one `summary` symbol");
        assert!(matches!(summaries[0].sym_kind, SymKind::Method));
    }

    #[test]
    fn extracts_namespace_declaration() {
        let out = extract(&php("namespace App\\Orders;\n\nfunction f() {}\n"));
        assert_eq!(out.declared_namespace.as_deref(), Some("App\\Orders"));
    }

    #[test]
    fn no_namespace_declaration_is_none() {
        let out = extract(&php("function f() {}\n"));
        assert_eq!(out.declared_namespace, None);
    }

    #[test]
    fn extracts_plain_and_aliased_use() {
        let out = extract(&php(
            "use App\\Orders\\Order;\nuse App\\Orders\\Handler as H;\n",
        ));
        let qualified: Vec<(&str, &str)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Qualified { fqn, bound_name } => {
                    Some((fqn.as_str(), bound_name.as_str()))
                }
                _ => None,
            })
            .collect();
        assert!(qualified.contains(&("App\\Orders\\Order", "Order")));
        assert!(qualified.contains(&("App\\Orders\\Handler", "H")));
    }

    #[test]
    fn extracts_grouped_use_with_shared_prefix() {
        let out = extract(&php("use App\\Orders\\{Order, Handler as H};\n"));
        let qualified: Vec<(&str, &str)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Qualified { fqn, bound_name } => {
                    Some((fqn.as_str(), bound_name.as_str()))
                }
                _ => None,
            })
            .collect();
        assert!(qualified.contains(&("App\\Orders\\Order", "Order")));
        assert!(qualified.contains(&("App\\Orders\\Handler", "H")));
    }

    #[test]
    fn extracts_function_use_ignoring_the_function_keyword() {
        let out = extract(&php("use function App\\Orders\\parseOrder;\n"));
        let qualified: Vec<&str> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Qualified { fqn, .. } => Some(fqn.as_str()),
                _ => None,
            })
            .collect();
        assert!(qualified.contains(&"App\\Orders\\parseOrder"));
    }

    #[test]
    fn trait_use_inside_class_body_is_not_treated_as_an_import() {
        let out = extract(&php(
            "trait Loggable {}\nclass Order {\n    use Loggable;\n}\n",
        ));
        assert!(out.imports.is_empty());
    }

    #[test]
    fn extracts_all_four_call_shapes() {
        let out = extract(&php("function handle() {\n\
             \x20   validate();\n\
             \x20   $order->summary();\n\
             \x20   $order?->summary();\n\
             \x20   Order::fromArray();\n\
             }\n"));
        let names: Vec<&str> = out
            .call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(names.contains(&"validate"));
        assert!(names.iter().filter(|n| **n == "summary").count() >= 2);
        assert!(names.contains(&"fromArray"));
    }

    #[test]
    fn extracts_namespace_qualified_plain_call() {
        let out = extract(&php(
            "function handle() {\n    App\\Orders\\parseOrder();\n}\n",
        ));
        let names: Vec<&str> = out
            .call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(names.contains(&"parseOrder"));
    }
}
