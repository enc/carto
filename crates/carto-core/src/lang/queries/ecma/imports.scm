; Whole nodes captured; decomposition (relative-path vs. package
; specifier, named/default/namespace clause shapes, aliasing) happens
; in ecma.rs — the same "handled more legibly in plain Rust" choice
; every other language's imports.scm makes for its own import shapes.
;
; `export.from_stmt` only matches an `export_statement` that actually
; has a `source` field populated — i.e. a re-export (`export { foo }
; from './x'`, `export * from './x'`) — by construction: a plain
; `export function foo() {}` has no `source` field at all, so this
; pattern simply never matches it. Confirmed in scope (ADR-0013):
; unlike named/default/namespace imports, a re-export never introduces
; a local binding a call site in *this* file could use, so it
; contributes only the file-level dependency edge, no `ImportedName`.

(import_statement) @import.stmt

(export_statement
  source: (string)) @export.from_stmt
