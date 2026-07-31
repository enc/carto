; Whole nodes captured; `mod`-with-no-body filtering and `use`-tree
; walking (which segments are the root vs. the imported item name) happen
; in rust.rs — tree-sitter's negated-field query syntax and deeply nested
; use-tree shapes (use_list, use_wildcard, use_as_clause) are handled
; more legibly in plain Rust than as query-only patterns for this
; deliberately modest v1 (spec §5.3; see ADR-0008).

(mod_item) @mod.decl

(use_declaration) @use.decl
