; Type-position captures shared by JavaScript, TypeScript, and TSX
; (ADR-0029/0030), run for all three extractors the same way
; `calls.scm` already is. `class X extends Y` is deliberately NOT
; here, even though it's real in plain JS too — TS's grammar wraps the
; superclass in an intervening `extends_clause` node (so
; `class_heritage` can also hold a sibling `implements_clause`), while
; plain JS's `class_heritage` holds the superclass `expression`
; directly with no such wrapper at all. Neither shape's pattern
; compiles against the other grammar (`tree_sitter::Query::new` fails
; on a node kind the target grammar doesn't define — the same reason
; `symbols_ts.scm` is its own file, ecma.rs's own module doc) — so
; `types_ts.scm` and `types_js.scm` each carry their own
; grammar-correct version of this capture instead of one here.
;
; `new Foo(...)` IS safe to share: both grammars name this field
; `constructor:` identically. It's also genuinely new signal, not a
; double-representation of anything — `calls.scm` only matches
; `call_expression`, never `new_expression` at all, so `new Foo()`
; produces zero edges today regardless of language; this is the first
; thing in this extractor that captures object construction.

(new_expression
  constructor: (_) @type.pos)
