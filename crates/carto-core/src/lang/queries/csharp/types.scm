; Type-position captures (ADR-0029/0030): every place a type name is
; *referenced* rather than invoked — a base/interface clause, a field/
; local/parameter type, a return type, a cast/`typeof`/`as` operand, a
; generic constraint, and a generic type argument list (the shape a DI
; registration like `services.AddSingleton<IQueryJobStore, X>()` needs).
; Each pattern anchors on a distinct grammar field (`type:`, `returns:`,
; `right:`, ...), or on `base_list` itself (which has no `type:` field
; at all — its type children are unnamed/positional) — so no node can
; match two patterns here (CLAUDE.md's double-match gotcha). Every
; `field: (_)` capture uses the tree-sitter wildcard, not a literal
; `type` supertype name, deliberately: a query pattern naming a
; grammar *supertype* (`type`, C#'s own umbrella for `identifier`/
; `generic_name`/`qualified_name`/.../`tuple_type`) is never itself a
; concrete node in the tree, so matching depends on query-engine
; supertype-alias support this crate hasn't otherwise relied on;
; `(_)` sidesteps the question entirely and lets `csharp.rs`'s
; `collect_type_names` — which already has to switch on every one of
; those concrete subtype kinds to descend into generics/wrappers —
; do the one classification job in one place. Same reasoning for
; `base_list`: captured whole rather than per-child, so its one
; record-primary-constructor shape (`primary_constructor_base_type`,
; itself carrying its own `type:` field) is walked by the same
; descent instead of a second query pattern that would double-match
; a base_list containing one.

(base_list) @type.pos

(variable_declaration
  type: (_) @type.pos)

(parameter
  type: (_) @type.pos)

(method_declaration
  returns: (_) @type.pos)

(delegate_declaration
  type: (_) @type.pos)

(property_declaration
  type: (_) @type.pos)

(cast_expression
  type: (_) @type.pos)

(typeof_expression
  type: (_) @type.pos)

(as_expression
  right: (_) @type.pos)

(type_parameter_constraint
  type: (_) @type.pos)

(catch_declaration
  type: (_) @type.pos)

; Generic invocation/construction arguments — `Helper<T>()`,
; `services.AddSingleton<IQueryJobStore, X>()`,
; `new Dictionary<string, IQueryJobStore>()`. Anchored on the concrete
; `type_argument_list` node itself, not the surrounding
; `invocation_expression`/`object_creation_expression`, so these can't
; collide with `calls.scm`'s own patterns on the same outer nodes.
; `object_creation_expression`'s own non-generic `type:` head (`new
; Order()`, `new Acme.Orders.OrderParser()`) is deliberately NOT
; captured anywhere in this file — `calls.scm` already turns that into
; a `calls` edge; capturing it here too would double-represent one
; call site as both a call and a reference.

(invocation_expression
  function: (generic_name (type_argument_list) @type.pos))

(invocation_expression
  function: (member_access_expression
    name: (generic_name (type_argument_list) @type.pos)))

(object_creation_expression
  type: (generic_name (type_argument_list) @type.pos))
