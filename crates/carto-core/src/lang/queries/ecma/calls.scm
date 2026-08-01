; Plain calls (foo(...)) and member calls (x.foo(...), including both
; instance calls and module-namespace calls like `ns.foo()` from
; `import * as ns` — the grammar can't tell them apart, same situation
; Python's attribute calls are in, not Rust's/PHP's, since JS/TS's
; member_expression has no separate "static/scoped" call node the way
; PHP's scoped_call_expression does). Optional chaining (`x?.foo()`)
; needs no separate pattern: `?.` is carried on an `optional_chain`
; field alongside `object`/`property`, which doesn't change either
; field's own shape, so the member-call pattern below already matches
; it.

(call_expression
  function: (identifier) @call.name) @call.expr

(call_expression
  function: (member_expression
    property: (property_identifier) @call.name)) @call.expr
