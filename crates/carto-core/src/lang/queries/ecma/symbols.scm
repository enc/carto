; Shared across TypeScript, TSX, and JavaScript (ADR-0013) — patterns
; that reference only node kinds every one of the three grammars
; defines. Class-related patterns are *not* here: a class's `name`
; field is `identifier` in JavaScript but `type_identifier` in
; TypeScript/TSX, and `tree_sitter::Query::new` fails to compile a
; query that references a node kind a grammar doesn't define at all —
; even inside an unused alternation branch, since JS's grammar has no
; `type_identifier` node kind whatsoever. Class patterns live in
; `symbols_class_identifier.scm` (JavaScript) and
; `symbols_class_type_identifier.scm` (TypeScript/TSX) instead — the
; same reason `symbols_ts.scm`'s interface/enum/type-alias patterns
; can't live here either.
;
; `is_pub` is decided in ecma.rs by walking each matched item's own
; ancestry (direct `export_statement` wrapping) plus a file-wide scan
; of bare `export { foo, bar as baz };` clauses — not encoded here.

(function_declaration
  name: (identifier) @symbol.name) @symbol.function

; Top-level `const/let X = (...) => {}` / `function (...) {}` — both
; the bare and `export`-wrapped forms. Scoped to `program`'s direct
; children (a plain top-level statement, or one `export_statement`
; deep) so a callback passed to some other function's call three
; levels down is never captured — a structural constraint, not a
; runtime check.
(program
  (lexical_declaration
    (variable_declarator
      name: (identifier) @symbol.name
      value: [(arrow_function) (function_expression)]) @symbol.const_function))
(program
  (export_statement
    declaration: (lexical_declaration
      (variable_declarator
        name: (identifier) @symbol.name
        value: [(arrow_function) (function_expression)]) @symbol.const_function)))
