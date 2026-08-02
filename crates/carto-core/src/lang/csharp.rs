//! C# `LangExtractor` (the eighth language, the first added after spec
//! §5.2's v1 set closed — the scope amendment and C#'s mapping of spec
//! §5.3's resolution policy are recorded in
//! `docs/adr/0016-csharp-resolution-policy-mapping.md`). Native
//! tree-sitter grammar via `carto_grammars::csharp_language()`. Queries
//! live in `lang/queries/csharp/*.scm`, embedded via `include_str!` so
//! they're reviewable independently of this file (spec §5.2).
//!
//! C# is namespace-based like PHP (ADR-0012), so most of that
//! machinery transfers: `declared_namespace` (here `.`-separated —
//! [`LangExtractor::namespace_separator`] is this extractor's override)
//! feeds the same repo-wide FQN index. What's genuinely new is the
//! import model: a plain `using Acme.Orders;` names a *namespace* —
//! potentially many files — not one declaration, hence
//! `RawImport::NamespaceImport` and its per-declaring-file fan-out in
//! `resolve` (ADR-0016). Two user-confirmed judgment calls also live
//! here: `internal` counts as exported (`is_pub` — carto's "same
//! package" is already "same walked repo", which approximates one
//! assembly), and `new Foo(...)` object creation is captured as a call
//! site (with constructors extracted as methods to receive the edges).

use super::extractor::{ExtractOut, LangExtractor, RawCallSite, RawImport, RawSymbol};
use crate::graph::SymKind;
use crate::lang::Lang;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Node, Parser, Query, QueryCursor};

const SYMBOLS_QUERY: &str = include_str!("queries/csharp/symbols.scm");
const IMPORTS_QUERY: &str = include_str!("queries/csharp/imports.scm");
const CALLS_QUERY: &str = include_str!("queries/csharp/calls.scm");

pub struct CSharpExtractor;

impl LangExtractor for CSharpExtractor {
    fn lang(&self) -> Lang {
        Lang::CSharp
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["cs"]
    }

    fn origin(&self) -> &'static str {
        "lang-csharp@1"
    }

    fn namespace_separator(&self) -> &'static str {
        "."
    }

    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        let language = carto_grammars::csharp_language();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("tree-sitter-c-sharp's language must load into a fresh Parser");

        // A parse failure yields nothing extracted rather than a crash —
        // same honesty principle as every other extractor: extracting
        // nothing beats guessing from a broken tree.
        let Some(tree) = parser.parse(src, None) else {
            return ExtractOut::default();
        };
        let root = tree.root_node();

        let (imports, declared_namespace) = extract_imports_and_namespace(root, src);
        ExtractOut {
            symbols: extract_symbols(root, src),
            imports,
            call_sites: extract_call_sites(root, src),
            declared_namespace,
        }
    }
}

fn extract_symbols(root: Node, src: &[u8]) -> Vec<RawSymbol> {
    let language = carto_grammars::csharp_language();
    let query = Query::new(&language, SYMBOLS_QUERY).expect("symbols.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut item_node = None;
        let mut sym_kind = None;
        let mut is_field = false;

        for cap in m.captures {
            match names[cap.index as usize] {
                "symbol.name" => name_node = Some(cap.node),
                "symbol.class" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Class);
                }
                "symbol.interface" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Interface);
                }
                "symbol.struct" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Struct);
                }
                "symbol.enum" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Enum);
                }
                "symbol.record" => {
                    item_node = Some(cap.node);
                    // `record`/`record class` -> Class, `record struct`
                    // -> Struct — the node kind is record_declaration
                    // either way; only its own anonymous `struct` token
                    // distinguishes them.
                    sym_kind = Some(if has_anonymous_token(cap.node, "struct") {
                        SymKind::Struct
                    } else {
                        SymKind::Class
                    });
                }
                // A delegate declares a named function *type*, not an
                // invocable — SymKind::Type, like Go's non-struct/
                // non-interface type_specs.
                "symbol.delegate" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Type);
                }
                "symbol.method" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Method);
                }
                "symbol.field" => {
                    item_node = Some(cap.node);
                    is_field = true;
                }
                _ => {}
            }
        }

        if is_field {
            // Only `const` fields become symbols; one per declarator
            // (`const int A = 1, B = 2;`), sharing the declaration's
            // lines/signature — see symbols.scm's own comment.
            let Some(field) = item_node else { continue };
            if !modifier_texts(field, src).iter().any(|m| m == "const") {
                continue;
            }
            for name in const_declarator_names(field, src) {
                out.push(raw_symbol(field, &name, SymKind::Const, src));
            }
            continue;
        }

        let (Some(name_node), Some(item_node), Some(sym_kind)) = (name_node, item_node, sym_kind)
        else {
            continue;
        };
        let name = text(src, name_node);
        out.push(raw_symbol(item_node, &name, sym_kind, src));
    }
    out
}

/// Builds one `RawSymbol` from its declaration node. Methods,
/// constructors, and consts get a `Type.Member` qualified name from the
/// nearest enclosing type (C#'s own member-access spelling, the same
/// choice Go's `Order.Summary` makes; type declarations themselves stay
/// bare, nested or not — same as every other extractor's containers).
fn raw_symbol(item_node: Node, name: &str, sym_kind: SymKind, src: &[u8]) -> RawSymbol {
    let qualified_name = match sym_kind {
        SymKind::Method | SymKind::Const => match enclosing_type(item_node) {
            Some(ty) => match ty.child_by_field_name("name") {
                Some(ty_name) => format!("{}.{name}", text(src, ty_name)),
                None => name.to_string(),
            },
            None => name.to_string(),
        },
        _ => name.to_string(),
    };
    let sig_end = item_node
        .child_by_field_name("body")
        .map(|b| b.start_byte())
        .unwrap_or(item_node.end_byte());
    RawSymbol {
        name: name.to_string(),
        qualified_name,
        sym_kind,
        start_line: item_node.start_position().row as u32 + 1,
        end_line: item_node.end_position().row as u32 + 1,
        signature: text_range(src, item_node.start_byte(), sig_end),
        is_pub: is_pub(item_node, src),
    }
}

/// C#'s exportedness rule for carto's purposes (ADR-0016, confirmed
/// with the user): `public` *and* `internal` count as exported —
/// carto's "same package" already means "same walked repo", which
/// approximates one assembly, exactly the scope `internal` grants.
/// Only `private`/`protected` (and `private protected`, and the C# 11
/// `file` modifier) are non-exported. No access modifier at all falls
/// back to C#'s own contextual default: namespace-level types default
/// to `internal` (exported here), interface members default to
/// `public`, and everything else nested in a type defaults to
/// `private`.
fn is_pub(item_node: Node, src: &[u8]) -> bool {
    let mods = modifier_texts(item_node, src);
    if mods.iter().any(|m| m == "public" || m == "internal") {
        return true;
    }
    if mods
        .iter()
        .any(|m| m == "private" || m == "protected" || m == "file")
    {
        return false;
    }
    match enclosing_type(item_node) {
        None => true, // namespace-level: defaults to internal
        Some(ty) => ty.kind() == "interface_declaration",
    }
}

/// The texts of a declaration's own direct `modifier` children
/// (`public`, `static`, `const`, ...). Direct children only — a nested
/// declaration's modifiers must not leak into its container's.
fn modifier_texts(node: Node, src: &[u8]) -> Vec<String> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| c.kind() == "modifier")
        .map(|c| text(src, c))
        .collect()
}

/// The nearest ancestor that is itself a type declaration — the
/// context both `is_pub`'s no-modifier default and a member's
/// qualified name need. `None` for a namespace-level declaration.
fn enclosing_type(node: Node) -> Option<Node> {
    let mut cur = node.parent();
    while let Some(n) = cur {
        match n.kind() {
            "class_declaration"
            | "struct_declaration"
            | "record_declaration"
            | "interface_declaration"
            | "enum_declaration" => return Some(n),
            _ => cur = n.parent(),
        }
    }
    None
}

/// Whether `node` has a direct anonymous child token with this text-kind
/// (e.g. record_declaration's own `struct` keyword).
fn has_anonymous_token(node: Node, token: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|c| !c.is_named() && c.kind() == token)
}

/// One name per `variable_declarator` under a (const) field_declaration.
fn const_declarator_names(field: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = field.walk();
    for decl in field
        .children(&mut cursor)
        .filter(|c| c.kind() == "variable_declaration")
    {
        let mut inner = decl.walk();
        for declarator in decl
            .children(&mut inner)
            .filter(|c| c.kind() == "variable_declarator")
        {
            if let Some(name) = declarator.child_by_field_name("name") {
                out.push(text(src, name));
            }
        }
    }
    out
}

fn extract_imports_and_namespace(root: Node, src: &[u8]) -> (Vec<RawImport>, Option<String>) {
    let language = carto_grammars::csharp_language();
    let query = Query::new(&language, IMPORTS_QUERY).expect("imports.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut imports = Vec::new();
    let mut declared_namespace: Option<String> = None;
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            match names[cap.index as usize] {
                "import.using" => {
                    if let Some(imp) = decompose_using(cap.node, src) {
                        imports.push(imp);
                    }
                }
                // First declaration wins for the rare file with more
                // than one namespace block (ADR-0016); matches come
                // back in tree order.
                "namespace.name" if declared_namespace.is_none() => {
                    declared_namespace = Some(text(src, cap.node));
                }
                _ => {}
            }
        }
    }
    (imports, declared_namespace)
}

/// Decomposes one `using_directive` (see imports.scm's own comment):
///
/// - `using P = Acme.Orders.Parser;` — the alias form is the one shape
///   with a `name:` field (the alias); the target type is the directive's
///   remaining named child. Maps to `Qualified { fqn, bound_name: alias }`
///   — one FQN, one declaration, PHP's exact shape (ADR-0012), resolved
///   through the same index.
/// - `using static System.Math;` — names a *type* whose static members
///   come into scope. Also `Qualified` (a type FQN resolves through
///   `fqn_to_file`, a namespace index miss would just drop it), bound
///   to the FQN's last segment; the member-binding behavior itself is
///   not modeled (ADR-0016).
/// - `using Acme.Orders;` — a namespace, potentially many files:
///   `NamespaceImport`, `resolve`'s per-declaring-file fan-out.
fn decompose_using(using: Node, src: &[u8]) -> Option<RawImport> {
    if let Some(alias) = using.child_by_field_name("name") {
        let mut cursor = using.walk();
        let target = using
            .children(&mut cursor)
            .filter(|c| c.is_named() && c.id() != alias.id())
            .last()?;
        return Some(RawImport::Qualified {
            fqn: text(src, target),
            bound_name: text(src, alias),
        });
    }
    let mut cursor = using.walk();
    let path_node = using
        .children(&mut cursor)
        .filter(|c| c.is_named())
        .last()?;
    let path = text(src, path_node);
    if path.is_empty() {
        return None;
    }
    if has_anonymous_token(using, "static") {
        let bound = path.rsplit('.').next().unwrap_or(&path).to_string();
        return Some(RawImport::Qualified {
            fqn: path,
            bound_name: bound,
        });
    }
    Some(RawImport::NamespaceImport { path })
}

fn extract_call_sites(root: Node, src: &[u8]) -> Vec<RawCallSite> {
    let language = carto_grammars::csharp_language();
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
        CSharpExtractor.extract(src.as_bytes(), "Test.cs")
    }

    fn import_paths(out: &ExtractOut) -> Vec<(&'static str, String)> {
        out.imports
            .iter()
            .map(|i| match i {
                RawImport::NamespaceImport { path } => ("namespace", path.clone()),
                RawImport::Qualified { fqn, .. } => ("qualified", fqn.clone()),
                _ => panic!("unexpected RawImport variant for C#"),
            })
            .collect()
    }

    #[test]
    fn extracts_class_method_and_qualified_name() {
        let out = extract(
            "namespace Acme.Orders;\n\npublic class Order\n{\n    public string Summary()\n    {\n        return \"s\";\n    }\n}\n",
        );
        assert_eq!(out.declared_namespace.as_deref(), Some("Acme.Orders"));
        let class = out.symbols.iter().find(|s| s.name == "Order").unwrap();
        assert!(matches!(class.sym_kind, SymKind::Class));
        assert!(class.is_pub);
        assert!(class.signature.contains("public class Order"));
        assert!(!class.signature.contains("Summary"));
        let method = out.symbols.iter().find(|s| s.name == "Summary").unwrap();
        assert!(matches!(method.sym_kind, SymKind::Method));
        assert_eq!(method.qualified_name, "Order.Summary");
        assert!(method.is_pub);
    }

    #[test]
    fn internal_counts_as_exported_and_private_protected_do_not() {
        let out = extract(
            "internal class Repo\n{\n    internal void Save() { }\n    private void Flush() { }\n    protected void Hook() { }\n    private protected void Seal() { }\n}\n",
        );
        let is_pub = |name: &str| out.symbols.iter().find(|s| s.name == name).unwrap().is_pub;
        assert!(is_pub("Repo"));
        assert!(is_pub("Save"));
        assert!(!is_pub("Flush"));
        assert!(!is_pub("Hook"));
        assert!(!is_pub("Seal"));
    }

    #[test]
    fn no_modifier_defaults_follow_context() {
        // Namespace-level type: internal by default -> exported.
        // Member with no modifier: private by default -> not.
        // Interface member: public by default -> exported.
        let out = extract(
            "class Quiet\n{\n    void Helper() { }\n}\n\npublic interface IAudit\n{\n    void Audit();\n}\n",
        );
        let is_pub = |name: &str| out.symbols.iter().find(|s| s.name == name).unwrap().is_pub;
        assert!(is_pub("Quiet"));
        assert!(!is_pub("Helper"));
        assert!(is_pub("Audit"));
    }

    #[test]
    fn record_and_record_struct_get_the_right_sym_kind() {
        let out = extract(
            "public record OrderLine(string Sku);\npublic record struct Point(int X);\npublic record class Tag(string Name);\n",
        );
        let kind_of = |name: &str| {
            out.symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no symbol named {name}"))
                .sym_kind
        };
        assert!(matches!(kind_of("OrderLine"), SymKind::Class));
        assert!(matches!(kind_of("Point"), SymKind::Struct));
        assert!(matches!(kind_of("Tag"), SymKind::Class));
    }

    #[test]
    fn struct_enum_interface_delegate_kinds() {
        let out = extract(
            "public struct Money { }\npublic enum Status { Open }\npublic interface IAudit { }\npublic delegate void Handler(string s);\n",
        );
        let kind_of = |name: &str| {
            out.symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no symbol named {name}"))
                .sym_kind
        };
        assert!(matches!(kind_of("Money"), SymKind::Struct));
        assert!(matches!(kind_of("Status"), SymKind::Enum));
        assert!(matches!(kind_of("IAudit"), SymKind::Interface));
        assert!(matches!(kind_of("Handler"), SymKind::Type));
    }

    #[test]
    fn constructor_is_a_method_with_qualified_name() {
        let out = extract(
            "public class Order\n{\n    public Order(string id)\n    {\n        Validate(id);\n    }\n\n    private static void Validate(string id) { }\n}\n",
        );
        let ctor = out
            .symbols
            .iter()
            .find(|s| s.name == "Order" && matches!(s.sym_kind, SymKind::Method))
            .expect("constructor must be extracted as a method");
        assert_eq!(ctor.qualified_name, "Order.Order");
        // Its body's call is attributable to it (innermost containment
        // happens in resolve; here just confirm the call site exists).
        assert!(out.call_sites.iter().any(|c| c.callee_name == "Validate"));
    }

    #[test]
    fn const_fields_emit_one_symbol_per_name_and_non_const_fields_do_not() {
        let out = extract(
            "public class Order\n{\n    public const string DefaultStatus = \"open\", AltStatus = \"alt\";\n    private string id;\n}\n",
        );
        let consts: Vec<&RawSymbol> = out
            .symbols
            .iter()
            .filter(|s| matches!(s.sym_kind, SymKind::Const))
            .collect();
        let names: Vec<&str> = consts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["DefaultStatus", "AltStatus"]);
        assert_eq!(consts[0].qualified_name, "Order.DefaultStatus");
        assert!(consts[0].is_pub);
        assert!(out.symbols.iter().all(|s| s.name != "id"));
    }

    #[test]
    fn method_local_const_is_not_extracted() {
        let out = extract(
            "public class Order\n{\n    public void Run()\n    {\n        const int limit = 10;\n    }\n}\n",
        );
        assert!(
            out.symbols
                .iter()
                .all(|s| !matches!(s.sym_kind, SymKind::Const))
        );
    }

    #[test]
    fn block_namespace_is_extracted_too() {
        let out = extract("namespace Acme.Legacy\n{\n    public class Old { }\n}\n");
        assert_eq!(out.declared_namespace.as_deref(), Some("Acme.Legacy"));
        assert!(out.symbols.iter().any(|s| s.name == "Old"));
    }

    #[test]
    fn plain_using_becomes_namespace_import() {
        let out = extract("using System;\nusing Acme.Orders;\n");
        assert_eq!(
            import_paths(&out),
            vec![
                ("namespace", "System".to_string()),
                ("namespace", "Acme.Orders".to_string()),
            ]
        );
    }

    #[test]
    fn global_using_decomposes_like_a_plain_one() {
        let out = extract("global using System.Text;\n");
        assert_eq!(
            import_paths(&out),
            vec![("namespace", "System.Text".to_string())]
        );
    }

    #[test]
    fn alias_using_becomes_qualified_with_bound_alias() {
        let out = extract("using Parser = Acme.Orders.OrderParser;\n");
        match &out.imports[..] {
            [RawImport::Qualified { fqn, bound_name }] => {
                assert_eq!(fqn, "Acme.Orders.OrderParser");
                assert_eq!(bound_name, "Parser");
            }
            _ => panic!("expected exactly one Qualified import"),
        }
    }

    #[test]
    fn using_static_becomes_qualified_bound_to_last_segment() {
        let out = extract("using static System.Math;\n");
        match &out.imports[..] {
            [RawImport::Qualified { fqn, bound_name }] => {
                assert_eq!(fqn, "System.Math");
                assert_eq!(bound_name, "Math");
            }
            _ => panic!("expected exactly one Qualified import"),
        }
    }

    #[test]
    fn extracts_plain_member_generic_and_conditional_call_shapes() {
        let out = extract(
            "public class Order\n{\n    public void Run(Order o)\n    {\n        Validate();\n        o.Summary();\n        Acme.Orders.OrderUtils.Format(o);\n        Helper<int>(1);\n        o?.Refresh();\n    }\n}\n",
        );
        let names: Vec<&str> = out
            .call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Validate", "Summary", "Format", "Helper", "Refresh"]
        );
    }

    #[test]
    fn object_creation_is_a_call_site_named_after_the_type() {
        let out = extract(
            "public class Factory\n{\n    public object Make()\n    {\n        var a = new Order();\n        var b = new Acme.Orders.OrderParser();\n        var c = new List<int>();\n        return a;\n    }\n}\n",
        );
        let names: Vec<&str> = out
            .call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert_eq!(names, vec!["Order", "OrderParser", "List"]);
    }
}
