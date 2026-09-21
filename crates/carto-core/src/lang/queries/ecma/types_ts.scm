; TypeScript/TSX-only type-position captures (ADR-0029/0030) — run as a
; second query pass, only for those two extractors, the same split
; `symbols_ts.scm` already makes (plain JavaScript's grammar defines
; none of these node kinds at all, so a single shared query file
; genuinely cannot include this content — `tree_sitter::Query::new`
; fails to compile a reference to a node kind the target grammar
; doesn't define).
;
; `(type_annotation) @type.pos` is the single broadest pattern here —
; every parameter, property, variable, and return-type annotation in
; this grammar wraps its type in one `type_annotation` node regardless
; of context, so one unanchored pattern reaches all of them without
; needing a separate pattern per declaration kind. That breadth is
; exactly why `required_parameter`/`optional_parameter` (whose own
; `type:` field *is* a `type_annotation`) get a deliberate no-op in
; `ecma.rs`'s `collect_type_names` rather than a second, redundant
; recursion path when one of them shows up as a tuple-type member —
; see that function's own doc comment (CLAUDE.md's double-match
; gotcha, reachable here through two different capture paths to the
; same node rather than two literal query patterns).
;
; `as_expression`/`satisfies_expression` have no named field for their
; own `type` child in this grammar (`children: [expression, type]`,
; both positional) — captured whole, with `collect_type_names` taking
; the *last* named child rather than matching on a field name that
; doesn't exist here.

; `class X extends Y` / `class X extends Y implements Z` — TS wraps the
; superclass in its own `extends_clause` node (distinct from plain JS's
; `class_heritage`, which holds the superclass expression directly with
; no wrapper — see `types.scm`'s own comment; `types_js.scm` carries
; the JS-grammar-correct equivalent of this same capture).
(class_heritage
  (extends_clause
    value: (_) @type.pos))

(type_annotation) @type.pos

(implements_clause) @type.pos

(extends_type_clause
  type: (_) @type.pos)

(new_expression
  type_arguments: (type_arguments) @type.pos)

(call_expression
  type_arguments: (type_arguments) @type.pos)

(as_expression) @type.pos

(satisfies_expression) @type.pos
