; Whole using_directive nodes captured; per-directive decomposition
; (plain `using Acme.Orders;` -> RawImport::NamespaceImport, aliased
; `using P = Acme.Orders.Parser;` and `using static System.Math;` ->
; RawImport::Qualified) happens in csharp.rs — same "handled more
; legibly in plain Rust" choice PHP's imports.scm makes for its own
; `use` shapes. A `global using` parses as the same node kind with a
; leading `global` token and decomposes identically; its repo-wide
; scope effect is not modeled (ADR-0016).

(using_directive) @import.using

; The file's declared namespace (file-scoped `namespace Acme.Orders;`
; or the block form) — feeds ExtractOut::declared_namespace, the same
; role PHP's namespace_definition capture plays. First declaration wins
; for the rare multi-namespace file (ADR-0016).

(file_scoped_namespace_declaration
  name: (_) @namespace.name)

(namespace_declaration
  name: (_) @namespace.name)
