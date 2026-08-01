; Whole import statements captured; per-statement decomposition (module
; path, relative dot count, however many `name:` fields are present,
; aliases, wildcard exclusion) happens in python.rs — tree-sitter's
; query syntax doesn't cleanly express "count the dots in this node's
; text" or "collect a variable number of same-named field children,"
; so this is handled more legibly in plain Rust, the same choice Rust's
; own imports.scm made for its use-tree shapes.

(import_statement) @import.stmt

(import_from_statement) @import.from_stmt
