; Type-position captures (ADR-0029/0030): every place a type name is
; *referenced* rather than invoked — a struct/tuple-struct field, a
; return type, a parameter, a `let` annotation, an `impl`'s trait and
; Self type, a const/static's type, a generic bound (`<T: Bound>` and
; `where T: Bound`, both surfaced through the same `trait_bounds` node
; this grammar uses for either), and turbofish generic arguments. Every
; `field: (_)` capture uses the wildcard, not a literal `_type` name —
; Rust's own type grammar rule is a *hidden* supertype (leading
; underscore; per this crate's own node-types.json, its concrete
; subtype is what actually appears in the tree, never a `_type` node
; itself), so `(_)` is the only pattern that reliably matches every
; subtype without relying on query-engine supertype-alias support this
; crate hasn't otherwise used — same reasoning `queries/csharp/
; types.scm` documents for C#'s own (named, but equally never-
; concrete) `type` supertype. `rust.rs`'s `collect_type_names` walks
; each captured subtree down to head identifiers; this file only
; decides *where* to look.
;
; Each pattern anchors on a distinct field or, for `trait_bounds`, a
; node with no competing anchor elsewhere in this file — so no node can
; match two patterns (CLAUDE.md's double-match gotcha). Nothing here
; overlaps `imports.scm`'s own `(scoped_identifier)`/
; `(scoped_type_identifier)` capture either: that one feeds
; `RawImport::BareReference` (ADR-0024, a *module*-level "this file
; references an unimported crate path" signal), this one feeds
; `RawTypeRef` (a *symbol*-level "this declaration names that type"
; signal) — genuinely different questions, answered from the same
; source text without conflict since they populate different
; `ExtractOut` fields read by different resolution logic.

(field_declaration
  type: (_) @type.pos)

(ordered_field_declaration_list
  type: (_) @type.pos)

(function_item
  return_type: (_) @type.pos)

(parameter
  type: (_) @type.pos)

(let_declaration
  type: (_) @type.pos)

(impl_item
  trait: (_) @type.pos)

(impl_item
  type: (_) @type.pos)

(const_item
  type: (_) @type.pos)

(static_item
  type: (_) @type.pos)

(where_predicate
  left: (_) @type.pos)

(type_parameter
  default_type: (_) @type.pos)

; `<T: Bound>`'s own bound and `where T: Bound`'s both parse to this
; one node kind — one pattern covers both without per-context
; anchoring.
(trait_bounds) @type.pos

; Turbofish generic arguments, two distinct parse shapes (verified
; against real parse trees, not assumed): a plain-call turbofish
; (`parse::<Order>(...)`, `s.parse::<Order>()`) is a `generic_function`
; node; a turbofish on a path *segment* before a final associated-item
; access (`Vec::<Order>::new()`) instead puts a `generic_type` in the
; outer `scoped_identifier`'s own `path:` field — this grammar reuses
; `scoped_identifier` for `Type::assoc_fn` regardless of whether `Type`
; itself carries generic arguments. Both anchor on a concrete node
; distinct from anything `calls.scm` or `imports.scm`'s `path.ref`
; capture already matches on the same outer nodes.

(generic_function
  type_arguments: (type_arguments) @type.pos)

(scoped_identifier
  path: (generic_type) @type.pos)
