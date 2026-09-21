; Type-position captures (ADR-0029/0030): a parameter/return
; annotation, an annotated assignment (`x: Foo = ...`), and a class's
; base-class list. Every field capture uses the wildcard rather than a
; literal `type` node name for consistency with every other language's
; types.scm, even though Python's own `type` grammar rule is, unlike
; C#/Rust/TS's hidden equivalents, a genuine concrete wrapper node
; (verified against this crate's own node-types.json: it has real
; `children`, not `subtypes` — it *does* appear in the parse tree) —
; `python.rs`'s `collect_type_names` unwraps it the same way it unwraps
; any other single-child container, so the wildcard costs nothing and
; keeps the reasoning uniform across every language's query file.
;
; Each pattern anchors on a distinct field, so no node can match two
; patterns (CLAUDE.md's double-match gotcha) — and `collect_type_names`
; only ever recurses into a `generic_type`/`attribute`/`argument_list`
; node reached *from* one of these anchors, never from an unanchored
; global pattern, so ordinary runtime subscript/call expressions
; elsewhere in the file are never mistaken for a type reference. (A
; subscripted generic in annotation position, `dict[str, Foo]`, parses
; to `generic_type`, not `subscript` — verified against a real parse
; tree; see `python.rs`'s own `collect_type_names` doc comment.)

(typed_parameter
  type: (_) @type.pos)

(typed_default_parameter
  type: (_) @type.pos)

(function_definition
  return_type: (_) @type.pos)

(assignment
  type: (_) @type.pos)

(class_definition
  superclasses: (argument_list) @type.pos)
