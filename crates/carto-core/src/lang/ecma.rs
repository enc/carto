//! TypeScript, TSX, and JavaScript `LangExtractor`s (spec §5.2/§5.3 as
//! amended by ADR-0013). Native tree-sitter grammars via
//! `carto_grammars::{typescript,tsx,javascript}_language()`. Queries
//! live in `lang/queries/ecma/*.scm`, embedded via `include_str!` so
//! they're reviewable independently of this file (spec §5.2).
//!
//! Unlike Rust/Python/PHP (one module each), these three `Lang`
//! variants share this single module and most of one query set —
//! verified their `function_declaration`/`class_declaration`/
//! `import_statement`/`call_expression`/`method_definition` node
//! shapes are structurally identical across all three grammars. This
//! is a deliberate deviation from the "one `queries/<lang>/` dir per
//! language" convention: TypeScript, TSX, and JavaScript are dialects
//! of one grammar family, unlike Rust vs. Python vs. PHP, which are
//! genuinely distinct languages. What can't be one file:
//! `tree_sitter::Query::new` fails to compile a query referencing a
//! node kind a grammar doesn't define *at all* — even inside an
//! unused alternation branch — so TS-only node kinds (interface/enum/
//! type-alias) live in a separate `symbols_ts.scm`, run as a second
//! pass only for TypeScript/TSX; and a class's own `name` field is
//! `identifier` in JavaScript but `type_identifier` in TypeScript/TSX
//! (JS's grammar has no `type_identifier` node kind whatsoever), so
//! the class/method patterns split into `symbols_class_identifier.scm`
//! / `symbols_class_type_identifier.scm` rather than one alternation.
//!
//! Full §5.3 mapping — relative-path decomposition reusing
//! `RawImport::Relative` rather than a new variant, scoped-package
//! roots, `is_pub` via export-reachability instead of a single-node
//! check, default/namespace imports not feeding tier (b), re-exports
//! in scope — recorded in
//! `docs/adr/0013-typescript-javascript-resolution-policy-mapping.md`.

use super::extractor::{
    ExtractOut, ImportedName, LangExtractor, RawCallSite, RawImport, RawSymbol, RawTypeRef,
};
use crate::graph::SymKind;
use crate::lang::Lang;
use std::collections::BTreeSet;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

const SYMBOLS_QUERY: &str = include_str!("queries/ecma/symbols.scm");
const SYMBOLS_QUERY_CLASS_IDENTIFIER: &str =
    include_str!("queries/ecma/symbols_class_identifier.scm");
const SYMBOLS_QUERY_CLASS_TYPE_IDENTIFIER: &str =
    include_str!("queries/ecma/symbols_class_type_identifier.scm");
const SYMBOLS_QUERY_TS: &str = include_str!("queries/ecma/symbols_ts.scm");
const IMPORTS_QUERY: &str = include_str!("queries/ecma/imports.scm");
const CALLS_QUERY: &str = include_str!("queries/ecma/calls.scm");
const TYPES_QUERY: &str = include_str!("queries/ecma/types.scm");
const TYPES_QUERY_TS: &str = include_str!("queries/ecma/types_ts.scm");
const TYPES_QUERY_JS: &str = include_str!("queries/ecma/types_js.scm");

pub struct TypeScriptExtractor;

impl LangExtractor for TypeScriptExtractor {
    fn lang(&self) -> Lang {
        Lang::TypeScript
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "mts", "cts"]
    }
    fn origin(&self) -> &'static str {
        "lang-ts@1"
    }
    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        extract_with(carto_grammars::typescript_language(), src, true)
    }
}

pub struct TsxExtractor;

impl LangExtractor for TsxExtractor {
    fn lang(&self) -> Lang {
        Lang::Tsx
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["tsx"]
    }
    fn origin(&self) -> &'static str {
        "lang-tsx@1"
    }
    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        extract_with(carto_grammars::tsx_language(), src, true)
    }
}

pub struct JavaScriptExtractor;

impl LangExtractor for JavaScriptExtractor {
    fn lang(&self) -> Lang {
        Lang::JavaScript
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["js", "mjs", "cjs", "jsx"]
    }
    fn origin(&self) -> &'static str {
        "lang-js@1"
    }
    fn extract(&self, src: &[u8], _relpath: &str) -> ExtractOut {
        extract_with(carto_grammars::javascript_language(), src, false)
    }
}

fn extract_with(language: Language, src: &[u8], is_ts_family: bool) -> ExtractOut {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("ecma grammar must load into a fresh Parser");

    // A parse failure yields nothing extracted rather than a crash —
    // same honesty principle as every other extractor: extracting
    // nothing beats guessing from a broken tree.
    let Some(tree) = parser.parse(src, None) else {
        return ExtractOut::default();
    };
    let root = tree.root_node();

    ExtractOut {
        symbols: extract_symbols(root, src, &language, is_ts_family),
        imports: extract_imports(root, src, &language),
        call_sites: extract_call_sites(root, src, &language),
        literals: Vec::new(),
        // TS/JS's member/selector calls are captured, not excluded
        // (ADR-0013) — nothing to count here.
        uncaptured_call_sites: Vec::new(),
        type_refs: extract_type_refs(root, src, &language, is_ts_family),
        declared_namespace: None,
        terraform: None,
    }
}

fn extract_symbols(
    root: Node,
    src: &[u8],
    language: &Language,
    is_ts_family: bool,
) -> Vec<RawSymbol> {
    // A file-wide scan for bare `export { foo, bar as baz };` clauses
    // — `is_pub` for a top-level declaration needs this in addition to
    // checking whether the declaration itself is directly wrapped in
    // an `export_statement`, since TS/JS (unlike PHP) has real
    // non-exported module-private top-level declarations.
    let mut exported_names = BTreeSet::new();
    collect_exported_names(root, src, &mut exported_names);

    let mut out = Vec::new();
    run_symbols_query(
        SYMBOLS_QUERY,
        root,
        src,
        language,
        &exported_names,
        &mut out,
    );
    let class_query = if is_ts_family {
        SYMBOLS_QUERY_CLASS_TYPE_IDENTIFIER
    } else {
        SYMBOLS_QUERY_CLASS_IDENTIFIER
    };
    run_symbols_query(class_query, root, src, language, &exported_names, &mut out);
    if is_ts_family {
        run_symbols_query(
            SYMBOLS_QUERY_TS,
            root,
            src,
            language,
            &exported_names,
            &mut out,
        );
    }
    out
}

/// Collects every bare `export { ... };` clause's *local* names — the
/// `name` field of each `export_specifier`, not `alias` (the
/// external-facing name a re-export can rename to; irrelevant to
/// whether the local declaration itself is reachable). Only plain
/// `identifier` names are kept — a string-shaped or `default`-literal
/// specifier is skipped, same "extract nothing rather than guess"
/// precedent as everywhere else. Plain recursive descent rather than a
/// compiled query: a one-off structural scan, same choice
/// `body_start_byte`-style helpers elsewhere make for logic that's
/// awkward to express as a query pattern.
fn collect_exported_names(node: Node, src: &[u8], out: &mut BTreeSet<String>) {
    // A re-export (`export { foo } from './x'`) names symbols declared
    // in *another* file — its specifiers say nothing about whether a
    // local declaration that happens to share a name is reachable, so
    // the whole statement is skipped, not descended into.
    if node.kind() == "export_statement" && node.child_by_field_name("source").is_some() {
        return;
    }
    if node.kind() == "export_specifier" {
        if let Some(name_node) = node.child_by_field_name("name") {
            if name_node.kind() == "identifier" {
                out.insert(text(src, name_node));
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_exported_names(child, src, out);
    }
}

fn run_symbols_query(
    query_src: &str,
    root: Node,
    src: &[u8],
    language: &Language,
    exported_names: &BTreeSet<String>,
    out: &mut Vec<RawSymbol>,
) {
    let query = Query::new(language, query_src).expect("ecma symbols query must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
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
                // Top-level `const foo = () => {}` — a `variable_declarator`
                // node, not a `function_declaration`; still `SymKind::Function`
                // (spec §4.1 has no separate "const-bound function" kind).
                "symbol.const_function" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Function);
                }
                "symbol.interface" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Interface);
                }
                "symbol.enum" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Enum);
                }
                "symbol.type" => {
                    item_node = Some(cap.node);
                    sym_kind = Some(SymKind::Type);
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
        // JS/TS's own dot convention for referring to a class's member
        // (`Order.summary`) — spec §4.3 leaves qualified-name spelling
        // to the extractor; same spelling Python's own class.method
        // convention happens to use, for the same reason (neither
        // language has Rust's/PHP's "::").
        let qualified_name = match &owner {
            Some(c) => format!("{c}.{name}"),
            None => name.clone(),
        };

        let is_pub = if matches!(sym_kind, SymKind::Method) {
            method_is_pub(item_node, name_node, src)
        } else {
            is_directly_exported(item_node) || exported_names.contains(&name)
        };

        let start_line = item_node.start_position().row as u32 + 1;
        let end_line = item_node.end_position().row as u32 + 1;
        let sig_end = signature_end_byte(item_node);
        let signature = text_range(src, item_node.start_byte(), sig_end);

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
}

/// The byte offset where `item_node`'s body starts, for signature
/// truncation. `item_node` is usually a node with its own `body`
/// field directly; the one exception is a top-level const-arrow's
/// `variable_declarator` (`symbol.const_function`), which has no
/// `body` field of its own — its `value` field (the arrow/function
/// expression) does.
fn signature_end_byte(item_node: Node) -> usize {
    let body = if item_node.kind() == "variable_declarator" {
        item_node
            .child_by_field_name("value")
            .and_then(|v| v.child_by_field_name("body"))
    } else {
        item_node.child_by_field_name("body")
    };
    body.map(|b| b.start_byte()).unwrap_or(item_node.end_byte())
}

/// Whether a `method_definition` is exported for spec §5.3 rule 2
/// tiers (b)/(c). A name captured as `private_property_identifier`
/// (real JS/TS `#foo(){}` syntax, a hard language guarantee — not a
/// convention the way Python's underscore prefix is) is never
/// `is_pub` regardless of any modifier. Otherwise, TS's
/// `accessibility_modifier` (`private`/`protected`) behaves like
/// PHP's `visibility_modifier`: absent means public.
fn method_is_pub(item_node: Node, name_node: Node, src: &[u8]) -> bool {
    if name_node.kind() == "private_property_identifier" {
        return false;
    }
    let mut cursor = item_node.walk();
    let modifier = item_node
        .children(&mut cursor)
        .find(|c| c.kind() == "accessibility_modifier");
    match modifier {
        None => true,
        Some(m) => {
            let kw = text(src, m);
            kw != "private" && kw != "protected"
        }
    }
}

/// Whether `item_node`'s own ancestry hits an `export_statement`
/// directly — `export function foo() {}` / `export class X {}` /
/// `export const foo = () => {}` (the last one's `item_node` is the
/// nested `variable_declarator`, one hop further up through its
/// `lexical_declaration` parent; the walk handles both uniformly
/// without special-casing which item kind it started from). Stops at
/// `program` — the walk never needs to go further, and a bare
/// top-level declaration's parent chain reaches `program` without
/// ever passing through `export_statement`.
fn is_directly_exported(item_node: Node) -> bool {
    let mut cur = item_node;
    while let Some(parent) = cur.parent() {
        if parent.kind() == "export_statement" {
            return true;
        }
        if parent.kind() == "program" {
            return false;
        }
        cur = parent;
    }
    false
}

fn extract_imports(root: Node, src: &[u8], language: &Language) -> Vec<RawImport> {
    let query = Query::new(language, IMPORTS_QUERY).expect("ecma imports.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            match names[cap.index as usize] {
                "import.stmt" => out.extend(decompose_import_statement(cap.node, src)),
                "export.from_stmt" => out.extend(decompose_reexport_statement(cap.node, src)),
                _ => {}
            }
        }
    }
    out
}

/// `import Foo from './x'` / `import { a, b as c } from './x'` /
/// `import * as ns from './x'` / `import './x'` (side-effect only) /
/// package-specifier forms of all of the above. `source` (the
/// specifier string) always produces exactly one `RawImport`; the
/// clause shape only affects which `ImportedName`s (if any) it
/// carries.
fn decompose_import_statement(node: Node, src: &[u8]) -> Vec<RawImport> {
    let Some(source) = node.child_by_field_name("source") else {
        return vec![];
    };
    let specifier = string_literal_text(src, source);

    let mut top_cursor = node.walk();
    let clause = node
        .children(&mut top_cursor)
        .find(|c| c.kind() == "import_clause");

    let mut imported_names = Vec::new();
    if let Some(clause) = clause {
        let mut cc = clause.walk();
        for child in clause.children(&mut cc) {
            match child.kind() {
                // A bare default-import binding (`import Foo from
                // './x'`) — the local name is the importer's own
                // choice, not a name the target file declares under
                // that spelling, so there's no verifiable
                // `declared_name` to pair it with. Contributes only
                // the file-level edge below, never a tier-(b)
                // candidate — see `RawImport::Relative`'s doc comment.
                "identifier" => {}
                "named_imports" => {
                    let mut nc = child.walk();
                    for spec in child
                        .children(&mut nc)
                        .filter(|c| c.kind() == "import_specifier")
                    {
                        if let Some(n) = extract_import_specifier(spec, src) {
                            imported_names.push(n);
                        }
                    }
                }
                // `import * as ns from './x'` — whole-module binding,
                // same "nothing to pair a declared name with" reasoning
                // as the default case.
                "namespace_import" => {}
                _ => {}
            }
        }
    }

    vec![make_relative_or_absolute(&specifier, imported_names)]
}

fn extract_import_specifier(spec: Node, src: &[u8]) -> Option<ImportedName> {
    let name_node = spec.child_by_field_name("name")?;
    if name_node.kind() != "identifier" {
        // A string-shaped name (`import { "exotic-name" as x }`) or
        // the bare `default` literal (`import { default as X }`) —
        // extract nothing rather than guess at a declared name that
        // doesn't syntactically exist here.
        return None;
    }
    let declared_name = text(src, name_node);
    let bound_name = spec
        .child_by_field_name("alias")
        .map(|a| text(src, a))
        .unwrap_or_else(|| declared_name.clone());
    Some(ImportedName {
        bound_name,
        declared_name,
    })
}

/// `export { foo, bar as baz } from './x'` / `export * from './x'`.
/// Deliberately produces **no** `ImportedName`s — a re-export doesn't
/// introduce a local binding a call site in *this* file could ever
/// use (it only forwards names onward to whoever imports from this
/// file), so only the file-level dependency edge is meaningful here.
fn decompose_reexport_statement(node: Node, src: &[u8]) -> Vec<RawImport> {
    let Some(source) = node.child_by_field_name("source") else {
        return vec![];
    };
    let specifier = string_literal_text(src, source);
    vec![make_relative_or_absolute(&specifier, Vec::new())]
}

/// Reuses the existing `RawImport::Relative`/`Absolute` shapes rather
/// than a new variant (ADR-0013) — a relative specifier's leading
/// `../` segments decompose directly into `levels_up`, and whatever's
/// left after stripping one leading `./`/`../` becomes `module_path`,
/// which may itself contain internal slashes (`"shared/utils"`);
/// `resolve::resolve_relative_import` already tolerates that as an
/// opaque suffix. Anything not starting with `.` is a bare package
/// specifier, `Absolute`.
fn make_relative_or_absolute(specifier: &str, imported_names: Vec<ImportedName>) -> RawImport {
    if specifier.starts_with('.') {
        let (levels_up, module_path) = parse_relative_specifier(specifier);
        RawImport::Relative {
            levels_up,
            module_path,
            imported_names,
        }
    } else {
        RawImport::Absolute {
            root: package_root(specifier),
            imported_names,
        }
    }
}

/// Decomposes a relative specifier (`"./orders"`, `"../orders"`,
/// `"../../shared/utils"`) into `(levels_up, module_path)`: each
/// leading `../` is one more level up (same convention Python's dot-
/// counting uses); a single leading `./` means zero levels up. What
/// remains after stripping the leading dots may itself contain
/// internal slashes (`"shared/utils"`), tolerated as an opaque suffix
/// by `resolve::resolve_relative_import` already.
fn parse_relative_specifier(specifier: &str) -> (u32, String) {
    let mut s = specifier;
    let mut levels_up = 0u32;
    loop {
        if let Some(rest) = s.strip_prefix("../") {
            levels_up += 1;
            s = rest;
        } else if let Some(rest) = s.strip_prefix("./") {
            s = rest;
            break;
        } else {
            break;
        }
    }
    (levels_up, s.trim_end_matches('/').to_string())
}

/// A bare package specifier's root — the whole first segment
/// (`"lodash"` from `"lodash/fp"`), except a scoped package
/// (`@scope/pkg`, `@scope/pkg/subpath`), which roots at its first
/// *two* `/`-separated segments (`"@scope/pkg"`) — npm's own scoping
/// convention, something none of Rust's/Python's/PHP's package
/// namespaces have an equivalent of.
fn package_root(specifier: &str) -> String {
    if let Some(rest) = specifier.strip_prefix('@') {
        let mut parts = rest.splitn(2, '/');
        let scope = parts.next().unwrap_or("");
        let name = parts.next().and_then(|r| r.split('/').next()).unwrap_or("");
        format!("@{scope}/{name}")
    } else {
        specifier.split('/').next().unwrap_or(specifier).to_string()
    }
}

fn extract_call_sites(root: Node, src: &[u8], language: &Language) -> Vec<RawCallSite> {
    let query = Query::new(language, CALLS_QUERY).expect("ecma calls.scm must compile");
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

/// ADR-0029: every identifier `types.scm` (and, for TS/TSX,
/// `types_ts.scm`) found in a type position, walked down to head
/// identifiers by [`collect_type_names`]. Mirrors `extract_symbols`'s
/// own shared-then-TS-only two-pass shape.
fn extract_type_refs(
    root: Node,
    src: &[u8],
    language: &Language,
    is_ts_family: bool,
) -> Vec<RawTypeRef> {
    let mut out = Vec::new();
    run_types_query(TYPES_QUERY, root, src, language, &mut out);
    if is_ts_family {
        run_types_query(TYPES_QUERY_TS, root, src, language, &mut out);
    } else {
        run_types_query(TYPES_QUERY_JS, root, src, language, &mut out);
    }
    out
}

fn run_types_query(
    query_src: &str,
    root: Node,
    src: &[u8],
    language: &Language,
    out: &mut Vec<RawTypeRef>,
) {
    let query = Query::new(language, query_src).expect("ecma types.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if names[cap.index as usize] == "type.pos" {
                collect_type_names(cap.node, src, out);
            }
        }
    }
}

/// Descends a captured type-position subtree down to the head
/// identifier(s) it names, recursing through every TS wrapper type
/// (union/intersection/parenthesized/optional/array/tuple/generic) and
/// the shared JS/TS container shapes `types.scm`/`types_ts.scm` capture
/// whole (`type_annotation`, `implements_clause`). A member path
/// (`ns.Foo`, both `new ns.Foo()` and `class X extends ns.Base`) yields
/// only its rightmost segment via `member_expression`'s own `property:`
/// field — the same rightmost-identifier policy `calls.scm`'s own
/// member-call capture already applies.
///
/// `required_parameter`/`optional_parameter` are a **deliberate no-op**
/// here, not an oversight: their own `type:` field is itself a
/// `type_annotation`, already reached independently by
/// `types_ts.scm`'s unanchored top-level pattern. A tuple type's own
/// named members (`[x: Foo]`) parse as these same two node kinds, so
/// without this no-op, recursing into a tuple's children would
/// double-count every named tuple member's type the same way an
/// unguarded Go `parameter_list` recursion would double-count a named
/// return value (see `queries/go/types.scm`'s own comment on the
/// identical hazard). `predefined_type` (`string`, `number`, `void`,
/// `any`, `unknown`, ...) has no symbol to reference and is silently
/// skipped, like every other language's builtin-type exclusion.
fn collect_type_names(node: Node, src: &[u8], out: &mut Vec<RawTypeRef>) {
    match node.kind() {
        "identifier"
        | "type_identifier"
        | "property_identifier"
        | "private_property_identifier" => out.push(RawTypeRef {
            name: text(src, node),
            line: node.start_position().row as u32 + 1,
        }),
        "member_expression" => {
            if let Some(prop) = node.child_by_field_name("property") {
                collect_type_names(prop, src, out);
            }
        }
        "nested_type_identifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                collect_type_names(name, src, out);
            }
        }
        "generic_type" => {
            if let Some(name) = node.child_by_field_name("name") {
                collect_type_names(name, src, out);
            }
            if let Some(args) = node.child_by_field_name("type_arguments") {
                collect_type_names(args, src, out);
            }
        }
        "as_expression" | "satisfies_expression" => {
            // No named field for the `type` operand in this grammar
            // (`children: [expression, type]`, both positional) — the
            // type is always the *last* named child.
            let mut cursor = node.walk();
            if let Some(last) = node.named_children(&mut cursor).last() {
                collect_type_names(last, src, out);
            }
        }
        "type_annotation" | "implements_clause" | "union_type" | "intersection_type"
        | "parenthesized_type" | "optional_type" | "rest_type" | "tuple_type" | "array_type"
        | "type_arguments" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_type_names(child, src, out);
            }
        }
        "required_parameter" | "optional_parameter" => {}
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

/// A `string` node's literal content with its surrounding quote
/// characters stripped — always exactly one ASCII byte each (`'` or
/// `"`; a module specifier is never a template literal).
fn string_literal_text(src: &[u8], node: Node) -> String {
    let full = text(src, node);
    if full.len() >= 2 {
        full[1..full.len() - 1].to_string()
    } else {
        full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(src: &str) -> ExtractOut {
        TypeScriptExtractor.extract(src.as_bytes(), "test.ts")
    }

    fn tsx(src: &str) -> ExtractOut {
        TsxExtractor.extract(src.as_bytes(), "test.tsx")
    }

    fn js(src: &str) -> ExtractOut {
        JavaScriptExtractor.extract(src.as_bytes(), "test.js")
    }

    fn type_ref_names(out: &ExtractOut) -> Vec<&str> {
        out.type_refs.iter().map(|t| t.name.as_str()).collect()
    }

    // ADR-0029: nothing in this extractor captured a type position
    // before this — `interface`/type-annotated params, `class X
    // extends Y`, and `new Foo()` (which `calls.scm` never captures at
    // all, in any of the three languages this file handles) all
    // produced zero edges.

    #[test]
    fn extracts_parameter_property_and_return_type_annotations() {
        let out = ts(
            "interface Order {}\n\nclass Handler {\n    order: Order;\n\n    take(o: Order): Order {\n        return o;\n    }\n}\n",
        );
        let names = type_ref_names(&out);
        assert_eq!(
            names.iter().filter(|&&n| n == "Order").count(),
            3,
            "field + parameter + return type, got {names:?}"
        );
    }

    #[test]
    fn variable_annotation_is_captured() {
        let out = ts("interface Order {}\n\nconst x: Order = {} as Order;\n");
        let names = type_ref_names(&out);
        // The `: Order` annotation and the `as Order` assertion.
        assert_eq!(names.iter().filter(|&&n| n == "Order").count(), 2);
    }

    #[test]
    fn class_extends_is_captured_in_plain_javascript_too() {
        let out = js("class Base {}\n\nclass Derived extends Base {}\n");
        assert_eq!(type_ref_names(&out), vec!["Base"]);
    }

    #[test]
    fn new_expression_is_captured_in_plain_javascript_where_calls_scm_never_looked() {
        let out = js("class Order {}\n\nfunction make() {\n    return new Order();\n}\n");
        // calls.scm only matches call_expression, never new_expression
        // -- this must be genuinely new signal, not a duplicate of an
        // existing calls edge.
        assert!(out.call_sites.iter().all(|c| c.callee_name != "Order"));
        assert_eq!(type_ref_names(&out), vec!["Order"]);
    }

    #[test]
    fn generic_type_argument_and_implements_clause_are_captured() {
        let out = ts(
            "interface Comparable {}\ninterface Order {}\n\nclass Handler implements Comparable {\n    items: Array<Order>;\n}\n",
        );
        let names = type_ref_names(&out);
        assert!(names.contains(&"Comparable"));
        assert!(names.contains(&"Array"));
        assert!(names.contains(&"Order"));
    }

    #[test]
    fn generic_new_expression_and_generic_call_type_arguments_are_captured() {
        let out = ts(
            "interface Order {}\n\nfunction make() {\n    const m = new Map<string, Order>();\n    identity<Order>(m);\n}\n",
        );
        let names = type_ref_names(&out);
        assert_eq!(names.iter().filter(|&&n| n == "Order").count(), 2);
        assert!(names.contains(&"Map"));
    }

    #[test]
    fn interface_extends_is_captured() {
        let out = ts("interface Base {}\n\ninterface Derived extends Base {}\n");
        assert!(type_ref_names(&out).contains(&"Base"));
    }

    #[test]
    fn tsx_supports_the_same_type_annotation_capture_as_ts() {
        let out = tsx("interface Order {}\n\nfunction take(o: Order) {}\n");
        assert_eq!(type_ref_names(&out), vec!["Order"]);
    }

    #[test]
    fn plain_javascript_has_no_type_annotations_to_capture() {
        let out = js("function take(o) {}\n");
        assert!(type_ref_names(&out).is_empty());
    }

    #[test]
    fn predefined_types_are_not_captured() {
        let out = ts("function take(a: string, b: number, c: boolean): void {}\n");
        assert!(type_ref_names(&out).is_empty());
    }

    /// Regression guard for the double-match hazard `types_ts.scm`'s
    /// own comment documents: a tuple type's named member
    /// (`required_parameter`) must not be counted twice — once via
    /// `tuple_type`'s own descent, once via the independent, global
    /// `type_annotation` capture reaching the same node's `type:`
    /// field.
    #[test]
    fn named_tuple_member_is_not_double_counted() {
        let out = ts("interface Order {}\n\nfunction take(t: [x: Order, y: Order]) {}\n");
        let names = type_ref_names(&out);
        assert_eq!(
            names.iter().filter(|&&n| n == "Order").count(),
            2,
            "{names:?}"
        );
    }

    #[test]
    fn extracts_top_level_function() {
        let out = ts("export function parseOrder(input) {\n    return 1;\n}\n");
        assert_eq!(out.symbols.len(), 1);
        let sym = &out.symbols[0];
        assert_eq!(sym.name, "parseOrder");
        assert_eq!(sym.qualified_name, "parseOrder");
        assert!(matches!(sym.sym_kind, SymKind::Function));
        assert!(sym.is_pub);
        // The signature spans the `function_declaration` node's own
        // range, which does *not* include a wrapping `export` keyword
        // — unlike Rust's `pub`, which is a child of the item node
        // itself. `is_pub` (asserted above) is still correct; the
        // signature text just doesn't visually echo it. See ADR-0013.
        assert!(sym.signature.contains("function parseOrder(input)"));
        assert!(!sym.signature.contains("export"));
        assert!(!sym.signature.contains("return"));
    }

    #[test]
    fn non_exported_top_level_function_is_not_pub() {
        let out = ts("function helper(input) {\n    return input;\n}\n");
        assert!(!out.symbols[0].is_pub);
    }

    #[test]
    fn extracts_top_level_const_arrow_function_exported_and_bare() {
        let out = ts(
            "export const parseOrder = (input) => {\n    return 1;\n};\n\
             const helper = (input) => input;\n",
        );
        let names: Vec<(&str, bool, SymKind)> = out
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.is_pub, s.sym_kind))
            .collect();
        assert!(names.contains(&("parseOrder", true, SymKind::Function)));
        assert!(names.contains(&("helper", false, SymKind::Function)));
    }

    #[test]
    fn nested_arrow_function_is_not_captured_as_a_symbol() {
        // A callback passed to some other call is not a top-level
        // declaration — the query's `(program ...)` ancestor
        // constraint must exclude it.
        let out = ts("function outer() {\n    [1, 2].map((x) => x + 1);\n}\n");
        assert_eq!(out.symbols.len(), 1);
        assert_eq!(out.symbols[0].name, "outer");
    }

    #[test]
    fn bare_export_clause_makes_a_declaration_pub() {
        let out = ts("function helper() {}\nexport { helper };\n");
        assert!(out.symbols[0].is_pub);
    }

    #[test]
    fn export_clause_alias_does_not_affect_the_local_declarations_pub_status() {
        // `export { helper as h }` — the *local* name "helper" is what
        // must be checked, not the external-facing alias "h".
        let out = ts("function helper() {}\nexport { helper as h };\n");
        assert!(out.symbols[0].is_pub);
    }

    #[test]
    fn reexport_specifier_does_not_make_a_same_named_local_declaration_pub() {
        // `export { helper } from './other'` re-exports the *other*
        // file's `helper`; the local, never-exported `helper` must stay
        // module-private.
        let out = ts("function helper() {}\nexport { helper } from './other';\n");
        assert!(!out.symbols[0].is_pub);
    }

    #[test]
    fn extracts_class_and_public_method_with_qualified_name() {
        let out = ts("export class Order {\n    summary() {\n        return this.id;\n    }\n}\n");
        let class = out
            .symbols
            .iter()
            .find(|s| matches!(s.sym_kind, SymKind::Class))
            .expect("class symbol must be extracted");
        assert_eq!(class.name, "Order");
        assert!(class.is_pub);

        let method = out
            .symbols
            .iter()
            .find(|s| matches!(s.sym_kind, SymKind::Method))
            .expect("method symbol must be extracted");
        assert_eq!(method.name, "summary");
        assert_eq!(method.qualified_name, "Order.summary");
        assert!(method.is_pub);
    }

    #[test]
    fn method_is_extracted_exactly_once() {
        let out = ts("class Order {\n    summary() {\n        return 1;\n    }\n}\n");
        let summaries: Vec<&RawSymbol> =
            out.symbols.iter().filter(|s| s.name == "summary").collect();
        assert_eq!(summaries.len(), 1, "expected exactly one `summary` symbol");
    }

    #[test]
    fn ts_private_and_protected_methods_are_not_pub_but_public_and_bare_are() {
        let out = ts("class Order {\n\
             \x20   private refresh() {}\n\
             \x20   protected helper() {}\n\
             \x20   public summary() {}\n\
             \x20   bare() {}\n\
             }\n");
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
        assert!(
            is_pub("bare"),
            "no accessibility modifier defaults to public"
        );
    }

    #[test]
    fn js_hash_private_field_method_is_never_pub() {
        // `#refresh()` — a real JS/TS private-field guarantee, not a
        // convention; must never be `is_pub` even with no modifier
        // keyword at all (JS has no such keyword to begin with).
        let out = js("class Order {\n    #refresh() {}\n}\n");
        // The private_property_identifier's own text includes the `#`.
        let refresh = out
            .symbols
            .iter()
            .find(|s| s.name.starts_with('#'))
            .expect("private field method must still be extracted, just not is_pub");
        assert!(!refresh.is_pub);
    }

    #[test]
    fn extracts_interface_enum_and_type_alias() {
        let out = ts("export interface Auditable {\n    audit(): void;\n}\n\
             export enum Status {\n    Open,\n    Closed,\n}\n\
             export type Id = string;\n");
        let kinds: Vec<(&str, SymKind)> = out
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.sym_kind))
            .collect();
        assert!(kinds.contains(&("Auditable", SymKind::Interface)));
        assert!(kinds.contains(&("Status", SymKind::Enum)));
        assert!(kinds.contains(&("Id", SymKind::Type)));
    }

    #[test]
    fn javascript_extractor_does_not_capture_ts_only_constructs() {
        // Plain .js has no interface/enum/type-alias syntax at all;
        // this is really a "does the grammar even parse without
        // TS-only node kinds" smoke test.
        let out = js("export function f() {\n    return 1;\n}\n");
        assert_eq!(out.symbols.len(), 1);
    }

    #[test]
    fn tsx_extractor_parses_jsx_without_choking() {
        let out =
            tsx("export function Component() {\n    return <div className=\"x\">{1}</div>;\n}\n");
        assert_eq!(out.symbols.len(), 1);
        assert_eq!(out.symbols[0].name, "Component");
    }

    #[test]
    fn extracts_named_import_with_and_without_alias() {
        let out = ts("import { parseOrder, validate as v } from './orders';\n");
        let names: Vec<ImportedName> = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Relative { imported_names, .. } => Some(imported_names.clone()),
                _ => None,
            })
            .expect("relative import must be extracted");
        assert!(names.contains(&ImportedName {
            bound_name: "parseOrder".into(),
            declared_name: "parseOrder".into(),
        }));
        assert!(names.contains(&ImportedName {
            bound_name: "v".into(),
            declared_name: "validate".into(),
        }));
    }

    #[test]
    fn default_and_namespace_imports_produce_no_imported_names() {
        let out = ts("import Foo from './orders';\nimport * as ns from './handlers';\n");
        for imp in &out.imports {
            if let RawImport::Relative { imported_names, .. } = imp {
                assert!(
                    imported_names.is_empty(),
                    "default/namespace imports must not contribute an ImportedName"
                );
            }
        }
        assert_eq!(out.imports.len(), 2);
    }

    #[test]
    fn extracts_multi_segment_relative_path_with_levels_up() {
        let out = ts("import { thing } from '../../shared/orders/handlers';\n");
        let rel = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Relative {
                    levels_up,
                    module_path,
                    ..
                } => Some((*levels_up, module_path.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(rel.0, 2);
        assert_eq!(rel.1, "shared/orders/handlers");
    }

    #[test]
    fn extracts_same_directory_relative_import() {
        let out = ts("import { parseOrder } from './orders';\n");
        let rel = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Relative {
                    levels_up,
                    module_path,
                    ..
                } => Some((*levels_up, module_path.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(rel.0, 0);
        assert_eq!(rel.1, "orders");
    }

    #[test]
    fn extracts_bare_package_root() {
        let out = ts("import { z } from 'lodash/fp';\n");
        let root = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Absolute { root, .. } => Some(root.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(root, "lodash");
    }

    #[test]
    fn extracts_scoped_package_root() {
        let out = ts("import { Logger } from '@scope/pkg/logging';\n");
        let root = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Absolute { root, .. } => Some(root.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(root, "@scope/pkg");
    }

    #[test]
    fn reexport_produces_a_relative_import_with_no_imported_names() {
        let out = ts("export { parseOrder } from './orders';\n");
        let rel = out
            .imports
            .iter()
            .find_map(|i| match i {
                RawImport::Relative {
                    module_path,
                    imported_names,
                    ..
                } => Some((module_path.clone(), imported_names.clone())),
                _ => None,
            })
            .expect("re-export must produce a Relative import");
        assert_eq!(rel.0, "orders");
        assert!(rel.1.is_empty());
    }

    #[test]
    fn plain_export_declaration_is_not_treated_as_an_import() {
        let out = ts("export function f() {}\n");
        assert!(out.imports.is_empty());
    }

    #[test]
    fn extracts_plain_member_and_optional_chained_call_sites() {
        let out = ts("function handle() {\n\
             \x20   validate();\n\
             \x20   order.summary();\n\
             \x20   order?.summary();\n\
             }\n");
        let names: Vec<&str> = out
            .call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(names.contains(&"validate"));
        assert_eq!(
            names.iter().filter(|n| **n == "summary").count(),
            2,
            "both the plain and optional-chained member call must be captured"
        );
    }
}
