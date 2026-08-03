; Plain-identifier calls (foo(...)) and receiver.method(...) calls only
; are attempted for resolution (spec §5.3, "deliberately modest";
; ADR-0008). tree-sitter's `call_expression` node already excludes macro
; invocations (`println!()` parses as a distinct `macro_invocation`
; node), so no extra filtering is needed for that case.

(call_expression
  function: (identifier) @call.name) @call.expr

(call_expression
  function: (field_expression
    field: (field_identifier) @call.name)) @call.expr

; Path-qualified calls (Type::method(...), module::func(...)) — a
; `call_expression` whose `function` field is a `scoped_identifier` —
; are deliberately NEVER attempted for resolution (ADR-0008 unchanged).
; Captured separately, under `call.uncaptured`, purely so `resolve` can
; *count* them (ADR-0020, §1.1's honest absence signal) — never to
; produce an edge or feed any resolution tier.

(call_expression
  function: (scoped_identifier
    name: (identifier) @call.uncaptured)) @call.uncaptured.expr
