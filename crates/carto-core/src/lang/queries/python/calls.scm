; Plain-identifier calls (foo(...)) and attribute calls
; (receiver.method(...)) — captures just the attribute name.
;
; Unlike Rust, this also captures module-qualified calls (`pkg.func()`)
; the same way as instance/self calls (`self.method()`, `order.summary()`)
; — Python's grammar has no node type distinct from plain attribute
; access for a module-qualified call the way Rust's `scoped_identifier`
; is distinct from `field_expression`. There is no syntactic way to tell
; "pkg" is an imported module rather than an instance without semantic
; (type) information carto doesn't have in v1. This is a genuine
; difference from Rust's path-qualified-call exclusion, not the same
; exclusion re-applied — recorded in ADR-0011.
(call
  function: (identifier) @call.name) @call.expr

(call
  function: (attribute
    attribute: (identifier) @call.name)) @call.expr
