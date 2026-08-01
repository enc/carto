; Four call shapes, all captured — unlike Rust, which excludes
; path-qualified calls (`Type::method()`), and unlike Python, which
; can't tell them apart from instance calls in the first place. PHP's
; grammar *can* distinguish a static call (`scoped_call_expression`)
; from a plain or instance call the way Rust's `scoped_identifier` can,
; but static calls (`self::helper()`, factory methods) are pervasive in
; real PHP — excluding them the way ADR-0008 excludes Rust's would drop
; a large fraction of real call edges into invisibility, not even
; `unresolved_calls`. A deliberate divergence from ADR-0008's precedent,
; recorded in ADR-0012.

; Plain call: foo(...)
(function_call_expression
  function: (name) @call.name) @call.expr

; Namespace-qualified plain call: App\Orders\foo(...) — qualified_name
; has exactly one unnamed `name` child, the final segment.
(function_call_expression
  function: (qualified_name
    (name) @call.name)) @call.expr

; Instance call: $order->summary(...)
(member_call_expression
  name: (name) @call.name) @call.expr

; Nullsafe instance call: $order?->summary(...)
(nullsafe_member_call_expression
  name: (name) @call.name) @call.expr

; Static call: Order::fromArray(...), self::helper(...)
(scoped_call_expression
  name: (name) @call.name) @call.expr
