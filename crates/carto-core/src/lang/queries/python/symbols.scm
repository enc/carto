; Module/class-level definitions (spec §5.3: "deliberately modest" — no
; nested/local functions, no module-level assignments as Const: Python
; has no `const` keyword, and "this uppercase name is a constant" is a
; heuristic with no clear win for v1 — see ADR-0011).

(function_definition
  name: (identifier) @symbol.name) @symbol.function

(class_definition
  name: (identifier) @symbol.name) @symbol.class

; A decorated function/class keeps its decorator line(s) in its own
; range — @symbol.function/@symbol.class bind to the whole
; decorated_definition, not just the inner def, so a symbol's span
; always starts at its first decorator. The bare (undecorated) patterns
; above still match the *inner* function_definition/class_definition
; node here too (tree-sitter's generic patterns match anywhere,
; decorated or not) — python.rs drops that spurious inner match itself
; (byte-range dedup alone can't catch it: the inner and outer nodes have
; genuinely different ranges, unlike Rust's impl-block method case).
(decorated_definition
  definition: (function_definition
    name: (identifier) @symbol.name)) @symbol.function

(decorated_definition
  definition: (class_definition
    name: (identifier) @symbol.name)) @symbol.class

; Methods: a function_definition (decorated or not) directly inside a
; class's body. Each pattern below re-matches a node the patterns above
; already matched once (generically, as Function) — deduped by byte
; range in python.rs, preferring the Method classification, exactly the
; same fix Rust's impl-block methods needed.
(class_definition
  name: (identifier) @class.name
  body: (block
    (function_definition
      name: (identifier) @symbol.name) @symbol.method))

(class_definition
  name: (identifier) @class.name
  body: (block
    (decorated_definition
      definition: (function_definition
        name: (identifier) @symbol.name)) @symbol.method))
