# ts-app fixture

Synthetic package for the TypeScript/TSX/JavaScript extractors'
acceptance tests (spec §11.1, ADR-0013). Not real customer code.
Mixes `.ts`, `.tsx`, and `.js` files deliberately — proving all three
`Lang` variants work, and that they cross-resolve imports against each
other, realistic for a mixed repo — rather than spec §11.1's
illustrative "30 files."

| Path / call | Exercises |
|---|---|
| `src/orders.ts`'s `parseOrder` calling `validate` (same file) | resolution tier (a): same-file |
| `src/orders.ts`'s `export const auditOrder = (id) => {...}` | top-level arrow-function-as-const capture, `sym_kind: "function"` |
| `src/orders.ts`'s `export interface Order` | a TS-only symbol kind, `sym_kind: "interface"` |
| `src/orders.ts`'s `function internalHelper()` (no `export`) | a module-private top-level declaration — never `is_pub`, never a tier (b)/(c) candidate |
| `src/handlers/index.ts`'s `import { parseOrder, validate as v } from '../orders'` | a relative import one directory up — `levels_up: 1`, `module_path: "orders"` |
| `Handler.handle` calling `parseOrder(input)` | resolution tier (b): imported into the file |
| `Handler.handle` calling `v(input)` (the **aliased** import) | resolution tier (b) **through the alias** — proves ADR-0013's alias-resolution fix end-to-end, not just unit-tested; `v` was never resolvable before that fix |
| `src/handlers/index.ts`'s `import { logOrder } from '../shared/logging'` | a relative import crossing directories with a multi-segment `module_path` (`"shared/logging"`) — one level up, then back down into a sibling subdirectory |
| `Handler.handle` calling `logOrder(order.id)` | resolution tier (b), the multi-segment import above |
| `Handler.handle` calling `auditOrder(order.id)` (not imported, defined in another file) | resolution tier (c): same-package (unambiguous) |
| `src/handlers/index.ts`'s `import { Logger } from '@scope/pkg/logging'` | a scoped-package external import — `root: "@scope/pkg"`, not `"@scope"` or `"@scope/pkg/logging"` |
| `Handler.handle` calling `internalHelper()` | the module-private function above — not `is_pub`, so no tier (b)/(c) candidate exists — lands in `handle`'s `unresolved_calls` |
| `Handler.handle` calling `unknownExternalCall()` | the deliberate honesty-path case: no candidate anywhere ⇒ no edge, `unresolved_calls` |
| `src/reexport.ts`'s `export { parseOrder } from './orders'` | a re-export — a `certain` `imports` edge to `orders.ts`, same evidence (`mod-declaration`) Rust's bare `mod foo;` gets, since neither targets a specific name the way a named import does (ADR-0013) |
| `src/Component.tsx`'s `import { legacyHelper } from './legacy'` | a `.tsx` file importing a `.js` file — cross-`Lang`-variant resolution, extension-guessing finds `legacy.js` |
| `Component` calling `legacyHelper(1)` | resolution tier (b), proving `.tsx`'s own extractor and JSX parsing (`return <div>{value}</div>;`) don't interfere with normal extraction |

Verify with:

```bash
cargo run -p carto-cli -- index fixtures/ts-app --out /tmp/carto-ts --json
python3 -m json.tool /tmp/carto-ts/graph.json
```
