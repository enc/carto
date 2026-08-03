; Whole nodes captured; `mod`-with-no-body filtering and `use`-tree
; walking (which segments are the root vs. the imported item name,
; including grouped/nested use_list shapes — §1.3, ADR-0022) happen in
; rust.rs — tree-sitter's negated-field query syntax and deeply nested
; use-tree shapes are handled more legibly in plain Rust than as
; query-only patterns. use_wildcard and use_as_clause stay excluded
; (spec §5.3's "deliberately modest"; ADR-0008/ADR-0022).

(mod_item) @mod.decl

(use_declaration) @use.decl
