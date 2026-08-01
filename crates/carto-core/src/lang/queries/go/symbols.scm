; Package-level declarations only (spec §5.3: "deliberately modest" — no
; nested/local functions, consts, vars, or types declared inside a
; function body). Every pattern below is scoped to `(source_file ...)`
; direct children — Go allows `const`/`var`/`type` declarations inside a
; function body too (unlike Rust's `const`/`static`/`type`, which are
; always item-level), so unlike Rust's/Python's symbols.scm, that scoping
; is load-bearing here, not incidental. `function_declaration` and
; `method_declaration` need no such guard: Go has no nested named function
; declarations (only anonymous func literals, which this extractor
; doesn't capture at all, same "deliberately modest" precedent).

(source_file
  (function_declaration
    name: (identifier) @symbol.name) @symbol.function)

; Methods: the receiver's own type is a field of the method_declaration
; node itself (`receiver: (parameter_list (parameter_declaration type:
; ...)))`), not a separately-captured sibling the way PHP's class-name
; capture is — go.rs reads it directly via `child_by_field_name`, so no
; second capture is needed here.
(source_file
  (method_declaration
    name: (field_identifier) @symbol.name) @symbol.method)

; `type Order struct { ... }` / `type Auditable interface { ... }` /
; `type ID int` — go.rs inspects the spec's own `type:` field to decide
; SymKind::Struct vs. SymKind::Interface vs. SymKind::Type (ADR-0015);
; not encoded here, the same "handled more legibly in plain Rust" choice
; every other language's is_pub/sym_kind decision already makes.
(source_file
  (type_declaration
    (type_spec
      name: (type_identifier) @symbol.name) @symbol.type_spec))

; `type ID = int` — a type alias, always SymKind::Type regardless of
; what it aliases (an alias doesn't define a new struct/interface
; identity the way a `type_spec` does).
(source_file
  (type_declaration
    (type_alias
      name: (type_identifier) @symbol.name) @symbol.type_spec))

; `const A = 1` / `const A, B = 1, 2` / `const ( A = 1; B = 2 )` — the
; grouped/parenthesized form is still one-or-more direct `const_spec`
; children of `const_declaration`, and a single `const_spec` can itself
; declare more than one name (`name:` is a repeated field) — go.rs emits
; one RawSymbol per name, verified empirically against tree-sitter's
; actual capture behavior for a repeated field (not assumed).
(source_file
  (const_declaration
    (const_spec
      name: (identifier) @symbol.name) @symbol.const))

; `var Config = ...` / `var A, B int` / `var ( ... )` — same repeated-
; field shape as const_spec above.
(source_file
  (var_declaration
    (var_spec
      name: (identifier) @symbol.name) @symbol.var))
