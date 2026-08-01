; Whole `import_spec` nodes captured; per-spec decomposition (stripping
; the surrounding quotes from the `path` field's string-literal text; the
; optional `name:` field — a dot import, blank import, or explicit alias
; — is read but not threaded through, since `RawImport::PackagePath`
; carries no bound-name at all, see its own doc comment for why) happens
; in go.rs, the same "handled more legibly in plain Rust" choice every
; other language's imports.scm makes for its own import shapes.
;
; One pattern covers both the bare single-import form (`import "fmt"`,
; where `import_spec` is a direct child of `import_declaration`) and the
; parenthesized multi-import form (`import ( "fmt"; "os" )`, where each
; `import_spec` is a child of an intervening `import_spec_list`) —
; tree-sitter queries match a node by its own kind, not its parent's
; kind, so no separate pattern is needed for either shape.

(import_spec) @import.spec
