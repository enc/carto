; Plain calls (foo(...)) and selector calls (pkg.Func(...), obj.Method(...)
; — the grammar can't tell a package-qualified call from a method call
; apart, the same situation Python's/TS-JS's attribute/member calls are
; in, not Rust's exclusion (ADR-0008) or PHP's static-call-specific node
; (ADR-0012): Go's selector_expression is one node kind for both. Both
; shapes are captured anyway, the PHP/TS precedent, since `pkg.Func()` is
; how essentially all cross-package Go calls are written — excluding
; selector calls the way Rust excludes path-qualified ones would drop
; most real Go call edges into invisibility.

(call_expression
  function: (identifier) @call.name) @call.expr

(call_expression
  function: (selector_expression
    field: (field_identifier) @call.name)) @call.expr
