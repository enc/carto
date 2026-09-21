; Whole nodes captured; `mod`-with-no-body filtering and `use`-tree
; walking (which segments are the root vs. the imported item name,
; including grouped/nested use_list shapes — §1.3, ADR-0022) happen in
; rust.rs — tree-sitter's negated-field query syntax and deeply nested
; use-tree shapes are handled more legibly in plain Rust than as
; query-only patterns. use_wildcard and use_as_clause stay excluded
; (spec §5.3's "deliberately modest"; ADR-0008/ADR-0022).

(mod_item) @mod.decl

(use_declaration) @use.decl

; Bare fully-qualified-path references with no `use`/`mod` bringing
; them into scope at all (`carto_core::Result<u8>` needs no `use
; carto_core;` — valid since Rust 2018) — ADR-0024. Deliberately broad:
; this also matches nodes already inside a `use_declaration`'s own
; argument tree, and a `call_expression`'s `Type::method()`-shaped
; callee (the same node shape as a genuine crate-rooted path, ADR-0008
; already excludes this from call resolution for the identical
; ambiguity) — both are filtered out in rust.rs, which also applies the
; lowercase-root heuristic that separates a plausible crate name from a
; local PascalCase type/trait/generic-parameter name. Query-only
; filtering can't express "not inside a use_declaration" or "not a
; call's own callee" cleanly, the same reason every other shape in this
; file is decomposed in plain Rust rather than here.

(scoped_identifier) @path.ref

(scoped_type_identifier) @path.ref
