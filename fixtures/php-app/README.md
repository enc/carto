# php-app fixture

Synthetic package for the PHP extractor's acceptance tests (spec §11.1,
amended §5.2 — ADR-0012). Not real customer code. Deliberately small —
just enough to independently exercise every spec §5.3 resolution outcome
PHP can reach, plus the class/method qualified-name path and the
repo-wide FQN index PHP's `namespace`/`use` model needs
(`docs/adr/0012-php-resolution-policy-mapping.md` records the
interpretation decisions this exercises):

| Path / call | Exercises |
|---|---|
| `src/Orders.php`'s `namespace App\Orders;` | feeds the repo-wide FQN index — `App\Orders\Order`, `App\Orders\parseOrder`, `App\Orders\validate` all become resolvable targets |
| `src/Handlers.php`'s `use App\Orders\Order;` | an FQN-index hit — `imports` edge, `certain`, evidence `namespace-import` |
| `src/Handlers.php`'s `use App\Missing\Thing;` | a known namespace root (`App`) with no file declaring that exact FQN — no edge, no node (honest omission, INV-8) |
| `src/Handlers.php`'s `use Psr\Log\LoggerInterface;` | an unknown root — external `Module` node, `imports` edge `certain`, evidence `external-package` |
| `src/Orders.php`'s `class Order` with `__construct`/`summary`/`fromArray`/`refresh` | class + method extraction, `sym_kind: "class"`/`"method"`, qualified ID (`Order::summary`) |
| `parseOrder` calling `validate` (same file) | resolution tier (a): same-file |
| `Handler::handle` calling `parseOrder` (`use function App\Orders\parseOrder;`) | resolution tier (b): imported into the file |
| `Handler::handle` calling `auditOrder` (not imported, defined in another file) | resolution tier (c): same-package (unambiguous) |
| `Handler::handle` calling `$order->summary()` | resolution tier (c) again — a member call |
| `Handler::handle` calling `Order::fromArray()` | resolution tier (c) via a **static** call — PHP's grammar *can* distinguish this from a member call the way Rust's `scoped_identifier` can, but it's captured anyway (unlike Rust's exclusion) since static calls are pervasive in real PHP; see ADR-0012 |
| `Handler::handle` calling `$order->refresh()` | `refresh` is `private` — not `is_pub`, so no tier (b)/(c) candidate exists — lands in `handle`'s `unresolved_calls` |
| `Handler::handle` calling `$input->strip()` | a real call-site that never resolves — no symbol carto sees is named `strip` — `unresolved_calls`, same honesty principle as the Python fixture's `input.strip()` case |
| `Handler::handle` calling `unknownExternalCall()` | the deliberate honesty-path case: no candidate anywhere ⇒ no edge, `unresolved_calls` |

Verify with:

```bash
cargo run -p carto-cli -- index fixtures/php-app --out /tmp/carto-php --json
python3 -m json.tool /tmp/carto-php/graph.json
```
