; Module/crate-level item declarations (spec §5.3: "deliberately modest" —
; no nested/local fns, closures, `let` bindings, or macro definitions).

(function_item
  name: (identifier) @symbol.name) @symbol.function

(struct_item
  name: (type_identifier) @symbol.name) @symbol.struct

(enum_item
  name: (type_identifier) @symbol.name) @symbol.enum

(trait_item
  name: (type_identifier) @symbol.name) @symbol.trait

(const_item
  name: (identifier) @symbol.name) @symbol.const

(static_item
  name: (identifier) @symbol.name) @symbol.const

(type_item
  name: (type_identifier) @symbol.name) @symbol.type

; impl-block methods, qualified against the enclosing type at
; ID-construction time (not captured here — see rust.rs).
(impl_item
  type: (_) @impl.type
  body: (declaration_list
    (function_item
      name: (identifier) @symbol.name) @symbol.method))
