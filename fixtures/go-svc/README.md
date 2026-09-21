# go-svc fixture

Synthetic package for the Go extractor's acceptance tests (spec §11.1,
ADR-0015 — the closing slice of M1.b.2b, the last language in spec
§5.2's v1 set). Not real customer code. Deliberately small — just
enough to independently exercise every spec §5.3 resolution outcome Go
can reach, plus the two judgment calls ADR-0015 records: `go.mod` is
committed for realism but deliberately *not* parsed, and Go's
directory-as-package visibility (tier a′) plus its module-qualified,
directory-shaped import paths (`RawImport::PackagePath`):

| Path / call | Exercises |
|---|---|
| `go.mod` | realism only — indexed as a plain `File` node; its `module` line is never read |
| `cmd/server/main.go`'s `import "github.com/acme/svc/internal/orders"` | a Go package-import — resolved by longest-suffix match against the walked `internal/orders` directory, no `go.mod` parsing; fans out to **two** `imports` edges, one per Go file in that directory (`order.go`, `helpers.go`) |
| `cmd/server/main.go`'s `import "fmt"` / `import _ "github.com/lib/pq"` | no walked directory's path matches either — external `Module` nodes keyed by the *full* import path (`fmt`, `github.com/lib/pq`), not a truncated root; the blank import (`_`) still produces a real dependency edge |
| `internal/orders/order.go`'s `type Order struct` with `Summary` method | struct + method extraction, `sym_kind: "struct"`/`"method"`, qualified ID (`Order.Summary`, Go's own selector spelling) |
| `internal/orders/order.go`'s `const DefaultStatus` / `var StatusOpen` | `sym_kind: "const"`/`"var"` |
| `internal/orders/helpers.go`'s `type Auditable interface` | `sym_kind: "interface"`, decided from the `type_spec`'s own `type:` field, not a second query pattern |
| `ParseOrder` calling `validate` (same file, `order.go`) | resolution tier (a): same-file |
| `ParseOrder` calling `normalize` (unexported, defined in `helpers.go`, same directory) | resolution tier (a′): same-directory — Go's own package-is-a-directory visibility; the case none of Rust/Python/PHP/TS/JS's tiers could reach on their own |
| `ParseOrder` calling `audit.AuditOrder` (`import "github.com/acme/svc/internal/audit"`) | resolution tier (c): same-package (repo-wide, unambiguous) — Go's package-imports never feed tier (b) at all (a `PackagePath` import binds a package name, not a symbol name; see `RawImport::PackagePath`'s own doc comment), so every cross-package Go call that resolves does so via tier (c), even though it's genuinely imported |
| `main` calling `orders.ParseOrder` (`cmd/server/main.go`) | resolution tier (c) again, across the `cmd/server` / `internal/orders` directory boundary |
| `main` calling `fmt.Println` | a real call-site that never resolves — no symbol carto sees is named `Println` — lands in `main`'s `unresolved_calls`, same honesty principle as every other fixture's stdlib-call case |
| `main` calling `unknownExternalCall()` | the deliberate honesty-path case: no candidate anywhere ⇒ no edge, `unresolved_calls` |
| `ParseOrder`'s own `*Order` return type, and `Summary`'s own `*Order` receiver | ADR-0029's `references` edges: `parameter_declaration`'s `type:` field is captured *unanchored* (matches anywhere, including a method's own `receiver:` parameter list) — both resolve tier (a) same-file, `EdgeKind::References`, evidence `type-reference:same-file` |

Verify with:

```bash
cargo run -p carto-cli -- index fixtures/go-svc --out /tmp/carto-go --json
python3 -m json.tool /tmp/carto-go/graph.json
```
