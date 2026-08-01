; TypeScript/TSX-only additions to symbols.scm — run as a second query
; pass, only for those two extractors (see ecma.rs). Plain JavaScript's
; grammar defines none of these three node kinds at all, so a single
; fully-shared symbols.scm genuinely cannot include this file's
; content (`tree_sitter::Query::new` fails to compile a reference to a
; node kind the target grammar doesn't define). ADR-0013.

(interface_declaration
  name: (type_identifier) @symbol.name) @symbol.interface

(enum_declaration
  name: (identifier) @symbol.name) @symbol.enum

(type_alias_declaration
  name: (type_identifier) @symbol.name) @symbol.type
