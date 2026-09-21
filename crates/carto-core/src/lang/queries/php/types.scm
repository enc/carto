; Type-position captures (ADR-0029/0030): every place a type name is
; *referenced* rather than invoked — a property/parameter/constructor-
; promoted-property type, a return type, a `catch` clause's exception
; type(s), a class/interface's `extends`/`implements` list, a trait's
; `use` (a genuinely different node kind from namespace `use` —
; `imports.scm`'s own comment notes it was never captured by anything
; until now), and object construction (`new Foo()`, which — like
; TS/JS's `new Foo()` — `calls.scm` never captures at all: only
; `function_call_expression`/`member_call_expression`/
; `nullsafe_member_call_expression`/`scoped_call_expression` are
; call-site shapes there, so this is genuinely new signal, not a
; double-representation of anything). Every `field: (_)` capture uses
; the wildcard, not a literal `type` supertype name — PHP's own `type`
; grammar rule is a *hidden* supertype (its concrete subtype is what
; actually appears in the tree, never a `type` node itself), the same
; reasoning `queries/csharp/types.scm`, `queries/rust/types.scm`, and
; `queries/ecma/types_ts.scm` all already document for their own
; languages' equivalents.
;
; `object_creation_expression` has no named field for the constructed
; type at all — `name`/`qualified_name`/`relative_name` are unnamed
; alternatives among its many possible children (arguments, a dynamic
; class-name expression, ...), so each is matched by its own concrete
; node kind directly rather than a field anchor; a query pattern
; matches only *direct* children by default, so this can't accidentally
; reach into a constructor argument's own nested names.

(property_declaration
  type: (_) @type.pos)

(simple_parameter
  type: (_) @type.pos)

(property_promotion_parameter
  type: (_) @type.pos)

(function_definition
  return_type: (_) @type.pos)

(method_declaration
  return_type: (_) @type.pos)

(catch_clause
  type: (_) @type.pos)

(base_clause) @type.pos

(class_interface_clause) @type.pos

(use_declaration) @type.pos

(object_creation_expression
  (name) @type.pos)

(object_creation_expression
  (qualified_name) @type.pos)

(object_creation_expression
  (relative_name) @type.pos)
