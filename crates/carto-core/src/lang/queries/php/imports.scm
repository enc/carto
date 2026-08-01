; Whole nodes captured; per-statement decomposition (namespace path,
; which `use` shape — plain / aliased / function-or-const / grouped —
; and normalizing a leading `\`) happens in php.rs, same "handled more
; legibly in plain Rust" choice Rust's and Python's own imports.scm make
; for their own import shapes.
;
; A class body's `use SomeTrait;` (trait-use, not namespace-use) is a
; distinct node kind, `use_declaration` — never matched by the pattern
; below, which only fires on `namespace_use_declaration` — see ADR-0012.

(namespace_definition) @namespace.def

(namespace_use_declaration) @use.decl
