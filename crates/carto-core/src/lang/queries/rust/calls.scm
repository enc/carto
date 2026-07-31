; Plain-identifier calls (foo(...)) and receiver.method(...) calls only.
; Deliberately NOT captured (spec §5.3, "deliberately modest"; ADR-0008):
; path-qualified calls (Type::method(...), module::func(...)) — a
; `call_expression` whose `function` field is a `scoped_identifier`.
; tree-sitter's `call_expression` node already excludes macro invocations
; (`println!()` parses as a distinct `macro_invocation` node), so no
; extra filtering is needed for that case.

(call_expression
  function: (identifier) @call.name) @call.expr

(call_expression
  function: (field_expression
    field: (field_identifier) @call.name)) @call.expr
