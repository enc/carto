; Plain-JavaScript-only type-position capture (ADR-0029/0030): `class X
; extends Y`. Plain JS's grammar holds the superclass `expression`
; directly under `class_heritage`, with no intervening `extends_clause`
; wrapper the way TS's grammar has (TS's `class_heritage` can also hold
; a sibling `implements_clause`, which is why it needs the wrapper at
; all) — this pattern doesn't compile against the TS/TSX grammars, and
; `types_ts.scm` carries their grammar-correct equivalent instead. See
; `types.scm`'s own comment for the full reasoning; this file mirrors
; that split the same way `symbols_ts.scm` already established one for
; symbol capture.

(class_heritage
  (expression) @type.pos)
