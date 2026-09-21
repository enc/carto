//! Rust `LangExtractor` (spec §5.2/§5.3). Native tree-sitter grammar via
//! `carto_grammars::rust_language()`. Queries live in
//! `lang/queries/rust/*.scm`, embedded via `include_str!` so they're
//! reviewable independently of this file (spec §5.2).

use super::extractor::{
    ExtractOut, ImportedName, LangExtractor, RawCallSite, RawImport, RawSymbol, RawTypeRef,
};
use crate::graph::SymKind;
use crate::lang::Lang;
use std::collections::{BTreeMap, BTreeSet};
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Node, Parser, Query, QueryCursor};

const SYMBOLS_QUERY: &str = include_str!("queries/rust/symbols.scm");
const IMPORTS_QUERY: &str = include_str!("queries/rust/imports.scm");
const CALLS_QUERY: &str = include_str!("queries/rust/calls.scm");
const TYPES_QUERY: &str = include_str!("queries/rust/types.scm");

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

        let (call_sites, uncaptured_call_sites) = extract_call_sites(root, src);
        ExtractOut {
            symbols: extract_symbols(root, src),
            literals: Vec::new(),
            imports: extract_imports(root, src),
            call_sites,
            uncaptured_call_sites,
            type_refs: extract_type_refs(root, src),
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
        // ADR-0032: `owner` reuses `impl_type_node` — already computed
        // for `qualified_name` below, and exactly "the type this symbol
        // is declared in" that carto's owner-type disambiguation tier
        // needs.
        let owner = impl_type_node.map(|t| text(src, t));
        let qualified_name = match &owner {
            Some(t) => format!("{t}::{name}"),
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
                    owner,
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
    // Collected separately from `out`, since a bare reference's root
    // needs to be deduped across the *whole file* (§ADR-0024) before
    // becoming `RawImport`s — unlike `use.decl`, which already produces
    // one dedup pass per declaration.
    let mut bare_reference_roots: BTreeSet<String> = BTreeSet::new();
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
                        // One `use` declaration can name more than one
                        // root when it's a bare, path-less group (`use
                        // {crate::a, std::b};`, rare) — group the
                        // resulting pairs by root so the common case
                        // (`use a::{b, c};`, one shared root) still
                        // produces exactly one `RawImport`, same as a
                        // single-path `use` already has (§1.3, ADR-0022).
                        let mut by_root: BTreeMap<String, Vec<ImportedName>> = BTreeMap::new();
                        for (root_seg, imported_name) in walk_use_clause(arg, src) {
                            by_root.entry(root_seg).or_default().push(ImportedName {
                                declared_name: imported_name.clone(),
                                bound_name: imported_name,
                            });
                        }
                        for (root_seg, imported_names) in by_root {
                            out.push(RawImport::Absolute {
                                root: root_seg,
                                imported_names,
                            });
                        }
                    }
                }
                "path.ref" => {
                    if let Some(root_seg) = bare_reference_root(cap.node, src) {
                        bare_reference_roots.insert(root_seg);
                    }
                }
                _ => {}
            }
        }
    }
    for root_seg in bare_reference_roots {
        out.push(RawImport::BareReference { root: root_seg });
    }
    out
}

/// Whether `node` (a `scoped_identifier`/`scoped_type_identifier`
/// captured by `path.ref`) is a genuine bare-reference candidate for
/// §ADR-0024, and if so, its leftmost (root) segment. `None` for the
/// three excluded shapes: already inside a `use_declaration` (handled,
/// more precisely, by `walk_use_clause` above — capturing it here too
/// would just produce a redundant, differently-evidenced duplicate of
/// the same edge); a `call_expression`'s own callee (`Type::method()`,
/// ADR-0008's existing exclusion — the identical node shape as a
/// genuine crate-rooted path, so re-capturing it here would reintroduce
/// the ambiguity that exclusion exists to avoid); or a root that isn't
/// lowercase-leading (Rust's crate-naming convention — types/traits/
/// generic parameters are conventionally PascalCase, e.g. `Order`,
/// `Self`, `T` — the only signal available to separate a plausible
/// crate name from a local associated-item path without
/// cross-referencing the whole symbol table, which this extractor
/// deliberately doesn't do here).
fn bare_reference_root(node: Node, src: &[u8]) -> Option<String> {
    if is_inside_use_declaration(node) || is_call_callee(node) {
        return None;
    }
    let root_seg = leftmost_text(node, src);
    let first = root_seg.chars().next()?;
    if !first.is_lowercase() {
        return None;
    }
    // `crate`/`self`/`super` are keywords, not identifiers, so
    // `leftmost_text` already stops at them (see `walk_use_tree`'s own
    // handling) rather than descending further — excluded here anyway,
    // defensively, since they're internal by definition regardless.
    if matches!(root_seg.as_str(), "crate" | "self" | "super") {
        return None;
    }
    Some(root_seg)
}

/// Walks every ancestor of `node` (not just the immediate parent, since
/// a grouped `use a::{b::Type};`'s member sits several nodes below the
/// `use_declaration` itself — `use_list`, `scoped_use_list`, ...) —
/// `true` if any of them is a `use_declaration`.
fn is_inside_use_declaration(node: Node) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "use_declaration" {
            return true;
        }
        current = parent;
    }
    false
}

/// Whether `node` is (part of) a `call_expression`'s own callee.
/// `scoped_identifier`/`scoped_type_identifier` nest for a
/// multi-segment path (`std::mem::swap` contains `std::mem` as its own
/// `path` field) — walks up through that same-kind chain to the
/// outermost node representing the *whole* path expression first, then
/// checks whether *that* node's real parent is the `call_expression`
/// with it as the `function` field. Without this, `swap` in
/// `std::mem::swap(...)` would exclude the outer node but miss the
/// nested `std::mem` one, which would otherwise still pass every other
/// filter (lowercase root `std`) and produce a spurious bare-reference
/// import.
fn is_call_callee(node: Node) -> bool {
    let mut current = node;
    loop {
        let Some(parent) = current.parent() else {
            return false;
        };
        if matches!(
            parent.kind(),
            "scoped_identifier" | "scoped_type_identifier"
        ) {
            current = parent;
            continue;
        }
        return parent.kind() == "call_expression"
            && parent.child_by_field_name("function") == Some(current);
    }
}

/// Expands a `use` argument tree into zero or more `(root_segment,
/// imported_name)` pairs — the same shape a single-path `use` has
/// always produced, generalized to the two grouped shapes
/// tree-sitter-rust's grammar can nest under a `use` (§1.3, ADR-0022,
/// amending ADR-0008's original blanket `use_list` exclusion):
///
/// - `scoped_use_list` (`use a::{b, c};`, or nested `use a::{b::{c,
///   d}, e};`) — the group's own leftmost segment (`leftmost_text`)
///   becomes every member's `root_segment`; each member is walked via
///   [`walk_use_list_member`], threading that root through unchanged
///   at any nesting depth (a nested group's *own* path, e.g. `b` above,
///   only ever matters for resolving a `self` member inside it — see
///   that function).
/// - a bare `use_list` with no enclosing path at all (`use {a::b,
///   c::d};`, rare — only reachable when the whole `use` has no shared
///   prefix) — each member is independent, so this just delegates to
///   [`walk_use_list_member`] with no inherited root.
/// - anything else (a plain path, `use_as_clause`, `use_wildcard`) is
///   the pre-existing base case, [`walk_use_tree`].
fn walk_use_clause(node: Node, src: &[u8]) -> Vec<(String, String)> {
    match node.kind() {
        "scoped_use_list" => {
            let Some(list) = node.child_by_field_name("list") else {
                return Vec::new();
            };
            let root = node
                .child_by_field_name("path")
                .map(|p| leftmost_text(p, src));
            let group_last_segment = node
                .child_by_field_name("path")
                .map(|p| last_path_segment(p, src));
            let mut out = Vec::new();
            let mut list_cursor = list.walk();
            for member in list.named_children(&mut list_cursor) {
                out.extend(walk_use_list_member(
                    member,
                    src,
                    root.as_deref(),
                    group_last_segment.as_deref(),
                ));
            }
            out
        }
        "use_list" => {
            let mut out = Vec::new();
            let mut list_cursor = node.walk();
            for member in node.named_children(&mut list_cursor) {
                out.extend(walk_use_list_member(member, src, None, None));
            }
            out
        }
        _ => walk_use_tree(node, src).into_iter().collect(),
    }
}

/// One member of a `use_list` — reached only from [`walk_use_clause`].
/// `inherited_root` is the enclosing group's classification root
/// (`None` only for a bare path-less top-level `use_list`, in which
/// case a plain-path member resolves its own root via
/// [`walk_use_tree`] instead). `group_last_segment` is the *immediately
/// enclosing* group's own last path segment — used only to resolve a
/// `self` member (`use std::io::{self, Write};` imports `io` itself
/// alongside `Write`; a nested `use a::{b::{self, c}};` imports `b`
/// itself, not `a`, so this is deliberately the nearest enclosing
/// group's segment, not the outermost one, even though `inherited_root`
/// itself always stays the outermost).
fn walk_use_list_member(
    node: Node,
    src: &[u8],
    inherited_root: Option<&str>,
    group_last_segment: Option<&str>,
) -> Vec<(String, String)> {
    match node.kind() {
        "self" => match (inherited_root, group_last_segment) {
            (Some(root), Some(name)) => vec![(root.to_string(), name.to_string())],
            // A bare top-level `self` (no enclosing group) doesn't
            // parse as this node kind at all in valid Rust — stay
            // non-panicking and extract nothing rather than guess.
            _ => Vec::new(),
        },
        "scoped_use_list" => {
            // A nested group (`use a::{b::{c, d}, e};`): the *outer*
            // root threads through unchanged — recursing via
            // `walk_use_clause` here would incorrectly recompute a
            // deeper root from this node's own `path` (`b`), which
            // matters only for a `self` member directly inside it.
            let Some(list) = node.child_by_field_name("list") else {
                return Vec::new();
            };
            let nested_last_segment = node
                .child_by_field_name("path")
                .map(|p| last_path_segment(p, src));
            let mut out = Vec::new();
            let mut list_cursor = list.walk();
            for member in list.named_children(&mut list_cursor) {
                out.extend(walk_use_list_member(
                    member,
                    src,
                    inherited_root,
                    nested_last_segment.as_deref(),
                ));
            }
            out
        }
        // Still excluded, individually rather than dropping the whole
        // statement (ADR-0008/ADR-0022): no enumerable member list for
        // a wildcard, and an aliased member's own name is a separate,
        // smaller judgment call left out of this slice.
        "use_as_clause" | "use_wildcard" => Vec::new(),
        _ => match inherited_root {
            // A member with a real inherited root only ever contributes
            // its own *trailing* segment as the imported name (`b::c`
            // inside `use a::{b::c, d};` binds as `c`, the same
            // "root + leaf name only, middle segments dropped"
            // simplification a single-path `use` already made) — the
            // root for classification is always the group's, never
            // recomputed from the member itself.
            Some(root) => {
                let name = match node.kind() {
                    "scoped_identifier" => node
                        .child_by_field_name("name")
                        .map(|n| text(src, n))
                        .unwrap_or_else(|| text(src, node)),
                    _ => text(src, node),
                };
                vec![(root.to_string(), name)]
            }
            // No inherited root (a bare path-less `use_list` member) —
            // this member supplies its own complete path.
            None => walk_use_tree(node, src).into_iter().collect(),
        },
    }
}

/// Walks a plain `use` path (no grouping at all) to
/// `(root_segment, imported_name)` — both always present when this
/// returns `Some` at all. Handles plain paths (`use
/// crate::orders::parse_order;`, `use serde;`) at arbitrary depth by
/// following the `scoped_identifier` chain's `path` field to its
/// leftmost leaf. This is the base case [`walk_use_clause`] falls back
/// to for a non-grouped `use`; `use_wildcard` (`use a::*;`) and
/// `use_as_clause` (`use a::b as c;`) still return `None` here — no
/// enumerable member list for the former, and an aliased top-level
/// `use` is the same smaller judgment call left out as an aliased
/// group member (ADR-0008; grouping itself is no longer excluded,
/// ADR-0022).
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

/// The last (rightmost) segment of a plain `use` path node — `io` for
/// `std::io`, `a` for a bare `a`. Used only to resolve a `self` group
/// member, where the *bound name* is the enclosing group's own last
/// segment rather than the literal text `self`.
fn last_path_segment(node: Node, src: &[u8]) -> String {
    if node.kind() == "scoped_identifier" {
        if let Some(name) = node.child_by_field_name("name") {
            return text(src, name);
        }
    }
    text(src, node)
}

fn leftmost_text(node: Node, src: &[u8]) -> String {
    // `scoped_type_identifier` (a type-position path, e.g. `carto_core::
    // Result`, §ADR-0024's `bare_reference_root`) has the same `path`/
    // `name` field shape as `scoped_identifier` — a multi-segment type
    // path's own `path` field nests as a plain `scoped_identifier`
    // (only the final segment is a "type"), which the existing arm
    // below already recurses through.
    if matches!(node.kind(), "scoped_identifier" | "scoped_type_identifier") {
        if let Some(path) = node.child_by_field_name("path") {
            return leftmost_text(path, src);
        }
    }
    text(src, node)
}

/// Returns `(call_sites, uncaptured_call_sites)` — the plain-identifier/
/// method calls this extractor attempts to resolve, and the
/// path-qualified calls (`Type::method()`, `module::func()`) it counts
/// but never attempts to resolve (see `calls.scm`'s `call.uncaptured`
/// pattern; ADR-0020).
fn extract_call_sites(root: Node, src: &[u8]) -> (Vec<RawCallSite>, Vec<RawCallSite>) {
    let language = carto_grammars::rust_language();
    let query = Query::new(&language, CALLS_QUERY).expect("calls.scm must compile");
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();

    let mut out = Vec::new();
    let mut uncaptured = Vec::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let site = || RawCallSite {
                callee_name: text(src, cap.node),
                line: cap.node.start_position().row as u32 + 1,
            };
            match names[cap.index as usize] {
                "call.name" => out.push(site()),
                "call.uncaptured" => uncaptured.push(site()),
                _ => {}
            }
        }
    }
    (out, uncaptured)
}

/// ADR-0029: every identifier `types.scm` found in a type position,
/// walked down to head identifiers by [`collect_type_names`]. See that
/// function and `types.scm`'s own module comment for the position list
/// and why this is additive to, not a replacement for, ADR-0024's
/// existing `path.ref`/`RawImport::BareReference` capture.
fn extract_type_refs(root: Node, src: &[u8]) -> Vec<RawTypeRef> {
    let language = carto_grammars::rust_language();
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
/// identifier(s) it names, recursing through every wrapper Rust's
/// hidden `_type` supertype has (reference/pointer/array/tuple/
/// generic/bounded) and the container shapes `types.scm` captures
/// whole (`trait_bounds`, `type_arguments`, `ordered_field_declaration_
/// list`'s repeated `type:` field). A path type (`crate::orders::
/// Order`, `Type::AssocItem`) yields only its *rightmost* segment via
/// `scoped_type_identifier`'s own `name:` field — the same policy every
/// other extractor's qualified-call/qualified-type capture already
/// applies, so a type reference resolves through identical name-
/// matching semantics to a call. `primitive_type` (`u64`, `str`, ...),
/// `unit_type` (`()`), `never_type` (`!`), `metavariable`/
/// `macro_invocation` (macro-generated types, unresolvable
/// syntactically), `removed_trait_bound` (`?Sized`-family, obscure),
/// and `function_type` (`fn(T) -> U`, a rare enough shape that
/// descending into its own parameter/return types isn't worth the
/// complexity) are silently skipped — documented exclusions, not
/// oversights.
fn collect_type_names(node: Node, src: &[u8], out: &mut Vec<RawTypeRef>) {
    match node.kind() {
        "type_identifier" => out.push(RawTypeRef {
            name: text(src, node),
            line: node.start_position().row as u32 + 1,
        }),
        "scoped_type_identifier" => {
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
        "reference_type" | "pointer_type" => {
            if let Some(t) = node.child_by_field_name("type") {
                collect_type_names(t, src, out);
            }
        }
        "array_type" => {
            if let Some(t) = node.child_by_field_name("element") {
                collect_type_names(t, src, out);
            }
        }
        "abstract_type" | "dynamic_type" => {
            if let Some(t) = node.child_by_field_name("trait") {
                collect_type_names(t, src, out);
            }
        }
        "type_binding" => {
            if let Some(t) = node.child_by_field_name("type") {
                collect_type_names(t, src, out);
            }
        }
        "tuple_type" | "type_arguments" | "trait_bounds" | "bounded_type" => {
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
        RustExtractor.extract(src.as_bytes(), "test.rs")
    }

    fn type_ref_names(out: &ExtractOut) -> Vec<&str> {
        out.type_refs.iter().map(|t| t.name.as_str()).collect()
    }

    // ADR-0029: this crate's own first non-C# instance of the reported
    // gap — nothing in `RustExtractor` captured a type position at all
    // before this, so `deps SomeTrait --dir in` had no signal here
    // either. `field: T`, `-> T`, `param: T`, and `let x: T` are the
    // shapes every real Rust struct/impl leans on.
    #[test]
    fn extracts_type_refs_from_field_return_parameter_and_let_positions() {
        let out = extract(
            "struct Order;\n\nstruct Handler {\n    order: Order,\n}\n\nfn make() -> Order {\n    let x: Order = make_inner();\n    x\n}\n\nfn take(o: Order) {}\n\nfn make_inner() -> Order {\n    Order\n}\n",
        );
        let names = type_ref_names(&out);
        assert_eq!(
            names.iter().filter(|&&n| n == "Order").count(),
            5,
            "field + 2 return types + let annotation + parameter, got {names:?}"
        );
    }

    #[test]
    fn tuple_struct_fields_are_captured() {
        let out = extract("struct Order;\n\nstruct Wrapper(Order, i32);\n");
        let names = type_ref_names(&out);
        assert!(names.contains(&"Order"));
        assert!(!names.contains(&"i32"), "{names:?}"); // primitive_type, skip
    }

    #[test]
    fn generic_wrapper_yields_both_head_and_argument() {
        let out = extract("struct Order;\n\nfn make() -> Vec<Order> {\n    Vec::new()\n}\n");
        let names = type_ref_names(&out);
        assert!(names.contains(&"Vec"));
        assert!(names.contains(&"Order"));
    }

    #[test]
    fn reference_and_pointer_wrappers_recurse_to_the_inner_type() {
        let out = extract("struct Order;\n\nfn take(o: &Order, p: *const Order) {}\n");
        let names = type_ref_names(&out);
        assert_eq!(names.iter().filter(|&&n| n == "Order").count(), 2);
    }

    #[test]
    fn qualified_path_type_yields_only_the_rightmost_segment() {
        let out = extract(
            "mod orders {\n    pub struct Order;\n}\n\nfn take(o: crate::orders::Order) {}\n",
        );
        let names = type_ref_names(&out);
        assert_eq!(names, vec!["Order"]);
    }

    #[test]
    fn impl_block_trait_and_self_type_are_both_captured() {
        let out = extract("trait Greet {}\nstruct Order;\n\nimpl Greet for Order {}\n");
        let names = type_ref_names(&out);
        assert!(names.contains(&"Greet"));
        assert!(names.contains(&"Order"));
    }

    #[test]
    fn const_and_static_types_are_captured() {
        let out = extract(
            "struct Order;\n\nconst DEFAULT: Order = Order;\nstatic SHARED: Order = Order;\n",
        );
        let names = type_ref_names(&out);
        assert_eq!(names.iter().filter(|&&n| n == "Order").count(), 2);
    }

    #[test]
    fn generic_bound_and_where_clause_are_captured() {
        let out = extract(
            "trait Bound {}\n\nfn take<T: Bound>(t: T) {}\n\nfn take2<T>(t: T)\nwhere\n    T: Bound,\n{\n}\n",
        );
        let names = type_ref_names(&out);
        assert_eq!(
            names.iter().filter(|&&n| n == "Bound").count(),
            2,
            "<T: Bound> and where T: Bound, got {names:?}"
        );
    }

    #[test]
    fn turbofish_generic_argument_is_captured() {
        // `parse::<Order>(...)` -- the shape that's actually a
        // `generic_function` node in this grammar (verified against
        // the real parse tree, not assumed).
        let out = extract("struct Order;\n\nfn make() {\n    let _ = parse::<Order>(\"x\");\n}\n");
        let names = type_ref_names(&out);
        assert!(names.contains(&"Order"));
    }

    #[test]
    fn turbofish_on_a_path_segment_is_captured() {
        // `Vec::<Order>::new()` -- a genuinely different parse shape:
        // the turbofish sits on `Vec::<Order>` as a `generic_type`
        // inside the outer `scoped_identifier`'s own `path:` field, not
        // inside a `generic_function` node at all (this grammar reuses
        // `scoped_identifier` for `Type::assoc_fn` regardless of
        // whether `Type` itself carries generic arguments) — needs its
        // own pattern, distinct from the plain-call turbofish above.
        let out = extract("struct Order;\n\nfn make() {\n    let _ = Vec::<Order>::new();\n}\n");
        let names = type_ref_names(&out);
        assert!(names.contains(&"Order"));
    }

    #[test]
    fn primitive_and_unit_types_are_not_captured() {
        let out = extract("fn take(a: u64, b: &str, c: ()) {}\n");
        assert!(type_ref_names(&out).is_empty());
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
    fn use_list_is_now_extracted_but_wildcard_still_is_not() {
        // §1.3/ADR-0022: a grouped `use` used to extract nothing at
        // all — silent, not just reduced, information loss on any
        // fan-in/fan-out question whose only import to a crate happened
        // to be grouped. `use_wildcard` stays excluded (no enumerable
        // member list).
        let out = extract("use std::{fs, io};\nuse std::collections::*;\n");
        let uses: Vec<(&str, Vec<&str>)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Absolute {
                    root,
                    imported_names,
                } => Some((
                    root.as_str(),
                    imported_names
                        .iter()
                        .map(|n| n.bound_name.as_str())
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(uses.len(), 1, "only the grouped use, not the wildcard");
        assert_eq!(uses[0].0, "std");
        assert_eq!(uses[0].1, vec!["fs", "io"]);
    }

    #[test]
    fn nested_grouped_use_extracts_every_leaf_under_the_shared_root() {
        // `use a::{b::{c, d}, e};` — one shared root ("a"), three
        // imported names, the middle segment ("b") dropped the same way
        // a single-path use already drops non-leaf segments.
        let out = extract("use a::{b::{c, d}, e};\n");
        let uses: Vec<(&str, Vec<&str>)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Absolute {
                    root,
                    imported_names,
                } => Some((
                    root.as_str(),
                    imported_names
                        .iter()
                        .map(|n| n.bound_name.as_str())
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(uses.len(), 1, "one RawImport for the one shared root");
        assert_eq!(uses[0].0, "a");
        assert_eq!(uses[0].1, vec!["c", "d", "e"]);
    }

    #[test]
    fn self_in_a_group_imports_the_groups_own_last_segment() {
        // `use std::io::{self, Write};` imports `io` itself (bound as
        // "io") alongside `Write` — real, common Rust syntax.
        let out = extract("use std::io::{self, Write};\n");
        let uses: Vec<(&str, Vec<&str>)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Absolute {
                    root,
                    imported_names,
                } => Some((
                    root.as_str(),
                    imported_names
                        .iter()
                        .map(|n| n.bound_name.as_str())
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].0, "std");
        assert_eq!(uses[0].1, vec!["io", "Write"]); // source order: self, then Write
    }

    #[test]
    fn nested_self_imports_the_nested_groups_own_segment_not_the_outer_root() {
        // `use a::{b::{self, c}};` imports `a::b` (bound as "b") and
        // `a::b::c` (bound as "c") — the nested group's own last
        // segment, not the outermost root "a".
        let out = extract("use a::{b::{self, c}};\n");
        let uses: Vec<(&str, Vec<&str>)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Absolute {
                    root,
                    imported_names,
                } => Some((
                    root.as_str(),
                    imported_names
                        .iter()
                        .map(|n| n.bound_name.as_str())
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].0, "a");
        assert_eq!(uses[0].1, vec!["b", "c"]); // source order: self, then c
    }

    #[test]
    fn aliased_member_inside_a_group_is_skipped_individually() {
        // `use a::{b as c, d};` — the aliased member is dropped
        // (ADR-0008's exclusion, unchanged), but `d` still extracts —
        // strictly more than dropping the whole statement, never a
        // guess about what `b as c`'s bound name should be.
        let out = extract("use a::{b as c, d};\n");
        let uses: Vec<(&str, Vec<&str>)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Absolute {
                    root,
                    imported_names,
                } => Some((
                    root.as_str(),
                    imported_names
                        .iter()
                        .map(|n| n.bound_name.as_str())
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].0, "a");
        assert_eq!(uses[0].1, vec!["d"]);
    }

    #[test]
    fn bare_path_less_group_keeps_each_members_own_independent_root() {
        // `use {crate::a, std::b};` — no shared prefix at all; each
        // member supplies its own complete path, so this produces two
        // separate RawImports, not one.
        let out = extract("use {crate::a, std::b};\n");
        let mut uses: Vec<(&str, Vec<&str>)> = out
            .imports
            .iter()
            .filter_map(|i| match i {
                RawImport::Absolute {
                    root,
                    imported_names,
                } => Some((
                    root.as_str(),
                    imported_names
                        .iter()
                        .map(|n| n.bound_name.as_str())
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        uses.sort();
        assert_eq!(uses, vec![("crate", vec!["a"]), ("std", vec!["b"])]);
    }

    fn bare_reference_roots(out: &ExtractOut) -> Vec<&str> {
        out.imports
            .iter()
            .filter_map(|i| match i {
                RawImport::BareReference { root } => Some(root.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bare_reference_in_return_type_is_captured_with_no_use_statement() {
        // §ADR-0024's motivating case: main.rs's `-> carto_core::
        // Result<u8>` return type, no `use carto_core::...;` anywhere.
        let out = extract("fn run() -> carto_core::Result<u8> {\n    todo!()\n}\n");
        assert_eq!(bare_reference_roots(&out), vec!["carto_core"]);
    }

    #[test]
    fn type_method_call_shape_is_not_captured_as_a_bare_reference() {
        // `Order::new()` is the exact ambiguous shape ADR-0008 already
        // excludes from call resolution — must not become a spurious
        // "imports Order" edge either.
        let out = extract("fn handle() {\n    Order::new();\n}\n");
        assert!(bare_reference_roots(&out).is_empty());
    }

    #[test]
    fn multi_segment_call_callee_excludes_every_nested_segment() {
        // `std::mem::swap(...)` nests `std::mem` inside the outer
        // `std::mem::swap` node — both must be excluded, not just the
        // outermost one (the nested-ancestor-walk in `is_call_callee`).
        let out = extract("fn f() {\n    std::mem::swap(&mut a, &mut b);\n}\n");
        assert!(bare_reference_roots(&out).is_empty());
    }

    #[test]
    fn pascal_case_root_is_not_captured_as_a_bare_reference() {
        // `Self::Output`/`T::Item` — associated-type paths whose root
        // is a local type/generic parameter, not a crate. Rust's
        // crate-naming convention (lowercase) is the only signal
        // available to tell these apart without a full symbol
        // cross-reference.
        let out = extract(
            "trait Foo {\n    type Output;\n}\n\
             fn f<T: Foo>() -> Self::Output {\n    todo!()\n}\n\
             fn g<T: Foo>() -> T::Output {\n    todo!()\n}\n",
        );
        assert!(bare_reference_roots(&out).is_empty());
    }

    #[test]
    fn bare_reference_inside_a_use_declaration_is_not_duplicated() {
        // A real `use carto_core::Foo;` already produces an `Absolute`
        // import — the same inner path must not also surface as a
        // separate `BareReference` for the identical root.
        let out = extract("use carto_core::Foo;\n");
        assert!(bare_reference_roots(&out).is_empty());
        assert!(
            out.imports
                .iter()
                .any(|i| matches!(i, RawImport::Absolute { root, .. } if root == "carto_core"))
        );
    }

    #[test]
    fn bare_reference_root_is_deduped_across_the_whole_file() {
        let out = extract(
            "fn f() -> carto_core::Result<u8> {\n    todo!()\n}\n\
             fn g() -> carto_core::Result<u16> {\n    todo!()\n}\n",
        );
        assert_eq!(bare_reference_roots(&out), vec!["carto_core"]);
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
    fn path_qualified_calls_are_uncaptured_not_resolved() {
        // Still never attempted for resolution (ADR-0008) — but now
        // counted separately (ADR-0020, §1.1) rather than vanishing
        // entirely.
        let out = extract("fn handle() {\n    Order::new();\n    orders::save();\n}\n");
        assert!(out.call_sites.is_empty());
        let names: Vec<&str> = out
            .uncaptured_call_sites
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        assert!(names.contains(&"new"));
        assert!(names.contains(&"save"));
    }

    #[test]
    fn plain_and_uncaptured_calls_are_mutually_exclusive() {
        // A regression the query-level double-match gotcha (CLAUDE.md's
        // documented tree-sitter pitfall) would produce: the same call
        // site landing in both lists.
        let out =
            extract("fn handle() {\n    validate();\n    order.summary();\n    Order::new();\n}\n");
        assert_eq!(out.call_sites.len(), 2);
        assert_eq!(out.uncaptured_call_sites.len(), 1);
    }
}
