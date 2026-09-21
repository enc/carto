; Type-position captures (ADR-0029/0030): every place a type name is
; *referenced* rather than invoked — a struct field (including an
; embedded interface, which parses as a name-less `field_declaration`),
; a parameter, a function/method result, a `type` spec's underlying
; type, a type assertion/conversion operand, a `var`/`const`
; declaration's type, a composite literal's type, and an interface's
; own embedded-interface list. Every `field: (_)` capture uses the
; wildcard, not a literal `_type`/`_simple_type` supertype name — both
; are *hidden* rules in this grammar (leading underscore; their
; concrete subtype is what actually appears in the tree), the same
; reasoning `queries/csharp/types.scm` and `queries/rust/types.scm`
; both already document for their own languages' equivalents.
;
; `(field_declaration type: (_))` and `(parameter_declaration type:
; (_))` are deliberately **unanchored** — they match every occurrence
; anywhere in the file, not just inside a top-level `type`/`func`
; declaration, which is exactly what makes a second capture pattern
; unnecessary for a function/method's *named* multiple-return-value
; list (`func f() (a int, b string)`): that list is itself a
; `parameter_list` of `parameter_declaration` nodes, already reached by
; this same global pattern. `go.rs`'s `collect_type_names` therefore
; gives `parameter_list` a deliberate no-op when it shows up as a
; `result:` capture's own node — recursing into it there too would
; double-represent every named return type (CLAUDE.md's double-match
; gotcha, reachable through two different capture paths to the same
; node rather than two literal query patterns this time). The same
; reasoning excludes `struct_type` and `interface_type` from recursion
; entirely: their `field_declaration`/`type_elem` children are always
; independently, globally captured already, so descending into them
; from a `type_spec`/`composite_literal` capture would double-count.

(field_declaration
  type: (_) @type.pos)

(parameter_declaration
  type: (_) @type.pos)

(function_declaration
  result: (_) @type.pos)

(method_declaration
  result: (_) @type.pos)

(type_spec
  type: (_) @type.pos)

(type_assertion_expression
  type: (_) @type.pos)

(var_spec
  type: (_) @type.pos)

(const_spec
  type: (_) @type.pos)

(composite_literal
  type: (_) @type.pos)

(type_conversion_expression
  type: (_) @type.pos)

; Interface composition (`type Foo interface { Bar }`, `Bar` an
; embedded interface) — anchored on `interface_type`'s own direct
; `type_elem` children specifically, NOT a bare `(type_elem)` pattern:
; a generic type's own arguments (`Foo[Bar]`) are *also* wrapped in
; `type_elem` nodes, reached instead through `generic_type`'s
; `type_arguments` field in `collect_type_names`' own recursion. A
; global, unanchored `(type_elem)` pattern here would match those too
; and double-capture the same node through two different paths.
(interface_type
  (type_elem) @type.pos)
