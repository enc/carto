; JavaScript-only variant of the class/method patterns — a class's
; `name` field is a plain `identifier` in JS's grammar (TypeScript/TSX
; use `type_identifier` instead; see `symbols_class_type_identifier.scm`
; and `symbols.scm`'s doc comment for why these can't be one shared
; file with an alternation). Method patterns are otherwise identical
; to the TS/TSX variant — scoped to their `class_body`, the same "only
; ever nests inside its real container, no dedup needed" shape PHP's
; methods have. Two name-field patterns: a regular method's name is
; `property_identifier`; a real JS private field (`#foo(){}`) is
; `private_property_identifier` — a hard language guarantee ecma.rs's
; is_pub check keys off directly.

(class_declaration
  name: (identifier) @symbol.name) @symbol.class

(class_declaration
  name: (identifier) @class.name
  body: (class_body
    (method_definition
      name: (property_identifier) @symbol.name) @symbol.method))
(class_declaration
  name: (identifier) @class.name
  body: (class_body
    (method_definition
      name: (private_property_identifier) @symbol.name) @symbol.method))
