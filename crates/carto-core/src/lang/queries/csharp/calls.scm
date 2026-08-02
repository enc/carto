; Plain calls (Foo(...)), member/qualified calls (o.M(...),
; Acme.Orders.OrderUtils.Format(...) — one chained member_access whose
; `function:`-anchored outermost node is the only match), conditional-
; access calls (o?.M(...)), generic calls (Helper<int>(...)), and object
; creation (`new Order(...)` — captured per ADR-0016's user-confirmed
; judgment call: constructor invocation is a large share of real C#
; dependency structure, the same pervasiveness argument that made PHP
; capture static calls in ADR-0012 while Rust excludes path-qualified
; calls in ADR-0008). Like Go's selector capture (ADR-0015), the
; qualifier/receiver is discarded: only the rightmost identifier is the
; callee name.
;
; Each pattern anchors on a field (`function:`, `type:`, `name:`) whose
; value is exactly one of these node kinds, so no node can match two
; patterns — no dedup needed (CLAUDE.md's double-match gotcha).

(invocation_expression
  function: (identifier) @call.name)

(invocation_expression
  function: (member_access_expression
    name: (identifier) @call.name))

(invocation_expression
  function: (member_access_expression
    name: (generic_name (identifier) @call.name)))

(invocation_expression
  function: (generic_name (identifier) @call.name))

(invocation_expression
  function: (conditional_access_expression
    (member_binding_expression
      name: (identifier) @call.name)))

(invocation_expression
  function: (conditional_access_expression
    (member_binding_expression
      name: (generic_name (identifier) @call.name))))

(object_creation_expression
  type: (identifier) @call.name)

(object_creation_expression
  type: (qualified_name
    name: (identifier) @call.name))

(object_creation_expression
  type: (generic_name (identifier) @call.name))

(object_creation_expression
  type: (qualified_name
    name: (generic_name (identifier) @call.name)))
