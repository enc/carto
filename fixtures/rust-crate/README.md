# rust-crate fixture

Synthetic crate for M1.b.2a's Rust extractor acceptance tests (spec
§11.1). Not real customer code. Deliberately small — just enough to
independently exercise every spec §5.3 resolution outcome plus the impl-
block/qualified-name path (`docs/adr/0008-rust-resolution-policy-mapping.md`
records the interpretation decisions this exercises):

| Path / call | Exercises |
|---|---|
| `src/lib.rs`'s `pub mod handlers;` / `pub mod orders;` | `mod` declarations resolving to sibling files — `imports` edges, `certain` |
| `src/orders.rs`'s `impl Order { pub fn summary(..) }` | impl-block method extraction, `sym_kind: "method"`, qualified ID (`Order::summary`) |
| `parse_order` calling `validate` (same file) | resolution tier (a): same-file |
| `handle` calling `parse_order` (`use crate::orders::parse_order;`) | resolution tier (b): imported into the file |
| `handle` calling `audit_order` (no `use`, `pub` in another file) | resolution tier (c): same-package (unambiguous) |
| `handle` calling `order.summary()` (no `use`, `pub` in another file) | resolution tier (c) again, via the `field_expression`/method-call capture path rather than a plain identifier call |
| `validate` calling `input.is_empty()` | a real call-site that never resolves — `&str::is_empty` isn't a symbol carto ever sees, so it lands in `validate`'s `unresolved_calls`. Expected, not a bug: spec §5.3's name-based matching has no type information, so stdlib/external method calls are indistinguishable from unresolved user calls (INV-8: honest, not guessed) |
| `handle` calling `unknown_external_call()` | the deliberate honesty-path case: no candidate anywhere ⇒ no edge, recorded in `handle`'s `unresolved_calls` |

Verify with:

```bash
cargo run -p carto-cli -- index fixtures/rust-crate --out /tmp/carto-rust --json
python3 -m json.tool /tmp/carto-rust/graph.json
```
