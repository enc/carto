; Top-level declarations. Methods are captured separately below, scoped
; to their class-like parent, so there's no impl-block-style double
; match to dedupe here (unlike Rust/Python): `method_declaration` is a
; distinct node kind from `function_definition`, and only ever appears
; nested inside a class/interface/trait/enum body — see ADR-0012.

(function_definition
  name: (name) @symbol.name) @symbol.function

(class_declaration
  name: (name) @symbol.name) @symbol.class

(interface_declaration
  name: (name) @symbol.name) @symbol.interface

(trait_declaration
  name: (name) @symbol.name) @symbol.trait

(enum_declaration
  name: (name) @symbol.name) @symbol.enum

; Methods: one pattern per class-like container, each capturing the
; container's own name alongside the method so php.rs can build the
; `Class::method` qualified name (PHP's own separator — see ADR-0012).
; Anonymous classes have no `name:` field, so their methods never match
; any pattern here — a deliberate, documented exclusion, not a bug.

(class_declaration
  name: (name) @class.name
  body: (declaration_list
    (method_declaration
      name: (name) @symbol.name) @symbol.method))

(interface_declaration
  name: (name) @class.name
  body: (declaration_list
    (method_declaration
      name: (name) @symbol.name) @symbol.method))

(trait_declaration
  name: (name) @class.name
  body: (declaration_list
    (method_declaration
      name: (name) @symbol.name) @symbol.method))

(enum_declaration
  name: (name) @class.name
  body: (enum_declaration_list
    (method_declaration
      name: (name) @symbol.name) @symbol.method))
