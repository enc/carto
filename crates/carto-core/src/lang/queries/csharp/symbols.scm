; Type-level and member declarations (spec §5.3: "deliberately modest").
; Each container kind is a distinct node kind in this grammar (like
; PHP's, unlike Go's single type_spec), so one pattern per kind with no
; double-match/dedup risk. Not scoped to the compilation unit root:
; symbols legally live under a (file-scoped or block) namespace
; declaration, directly at the top level, or nested in another type —
; all captured. Local functions are a different node kind
; (local_function_statement) and are never matched — deliberately not
; extracted, like Go's function-local declarations.
;
; Not captured, documented in ADR-0016: properties (accessors, not
; invocables), events, indexers, operators, destructors, enum members.

(class_declaration
  name: (identifier) @symbol.name) @symbol.class

(interface_declaration
  name: (identifier) @symbol.name) @symbol.interface

(struct_declaration
  name: (identifier) @symbol.name) @symbol.struct

(enum_declaration
  name: (identifier) @symbol.name) @symbol.enum

; `record`/`record class` -> Class, `record struct` -> Struct — decided
; in csharp.rs from the node's own anonymous `struct` token, the same
; "handled more legibly in plain Rust" choice Go's type_spec_kind makes.
(record_declaration
  name: (identifier) @symbol.name) @symbol.record

(delegate_declaration
  name: (identifier) @symbol.name) @symbol.delegate

(method_declaration
  name: (identifier) @symbol.name) @symbol.method

; Constructors are extracted as methods (ADR-0016): `new Foo()` call
; sites are captured, and a constructor body's own calls need an
; innermost symbol to be attributed to.
(constructor_declaration
  name: (identifier) @symbol.name) @symbol.method

; Only `const` fields become symbols (SymKind::Const) — csharp.rs checks
; the modifier and emits one symbol per declarator (`const int A = 1,
; B = 2;`). Non-const fields are state, not API surface — not extracted,
; same modesty as Python's module-level assignments. A method-local
; `const` is a local_declaration_statement, a different node kind —
; never matched.
(field_declaration) @symbol.field
