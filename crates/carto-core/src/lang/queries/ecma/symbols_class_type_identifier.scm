; TypeScript/TSX-only variant of the class/method patterns — a class's
; `name` field is `type_identifier` in these two grammars (plain
; JavaScript uses `identifier` instead; see
; `symbols_class_identifier.scm` and `symbols.scm`'s doc comment for
; why these can't be one shared file with an alternation). Otherwise
; identical to the JavaScript variant.

(class_declaration
  name: (type_identifier) @symbol.name) @symbol.class

(class_declaration
  name: (type_identifier) @class.name
  body: (class_body
    (method_definition
      name: (property_identifier) @symbol.name) @symbol.method))
(class_declaration
  name: (type_identifier) @class.name
  body: (class_body
    (method_definition
      name: (private_property_identifier) @symbol.name) @symbol.method))
