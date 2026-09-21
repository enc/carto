# py-lib fixture

Synthetic package for M1.b.2b's Python extractor acceptance tests (spec
§11.1). Not real customer code. Deliberately small — just enough to
independently exercise every spec §5.3 resolution outcome plus the
class/method qualified-name path
(`docs/adr/0011-python-resolution-policy-mapping.md` records the
interpretation decisions this exercises):

| Path / call | Exercises |
|---|---|
| `pylib/handlers.py`'s `from .orders import parse_order` | a relative import (one dot = current package, `levels_up: 0`) resolving to a sibling file — `imports` edge, `certain` |
| `pylib/orders.py`'s `class Order` with `__init__`/`summary` methods | class + method extraction, `sym_kind: "class"`/`"method"`, qualified ID (`Order.summary`) |
| `parse_order` calling `validate` (same file) | resolution tier (a): same-file |
| `handle` calling `parse_order` (`from .orders import parse_order`) | resolution tier (b): imported into the file |
| `handle` calling `audit_order` (not imported, not underscore-prefixed, defined in another file) | resolution tier (c): same-package (unambiguous) |
| `handle` calling `order.summary()` (not imported, an attribute call) | resolution tier (c) again — Python's attribute-call syntax doesn't distinguish this from a module-qualified call the way Rust's `scoped_identifier` does (see ADR-0011); it resolves through the same tiers as any other attribute call |
| `handle` calling `input.strip()` | a real call-site that never resolves — `str.strip` isn't a symbol carto ever sees, so it lands in `handle`'s `unresolved_calls`. Expected, not a bug — same honesty principle as the Rust fixture's `input.is_empty()` case |
| `handle` calling `unknown_external_call()` | the deliberate honesty-path case: no candidate anywhere ⇒ no edge, recorded in `handle`'s `unresolved_calls` |
| `parse_order`'s own `-> Order` return annotation | ADR-0029's `references` edge: a `function_definition`'s `return_type:` field, resolved tier (a) same-file — `EdgeKind::References`, evidence `type-reference:same-file` |

Verify with:

```bash
cargo run -p carto-cli -- index fixtures/py-lib --out /tmp/carto-py --json
python3 -m json.tool /tmp/carto-py/graph.json
```
