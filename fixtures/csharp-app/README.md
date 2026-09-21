# csharp-app fixture

Synthetic app for the C# extractor's acceptance tests (spec §11.1,
ADR-0016 — the first language added after spec §5.2's v1 set closed).
Not real customer code. Deliberately small — just enough to
independently exercise every spec §5.3 resolution outcome C# can reach,
plus the judgment calls ADR-0016 records: `internal` counts as
exported, `new Foo()` is a call site resolving to the *type* (and
constructors are deliberately not symbols — extracting them would make
every `new` ambiguous), a plain `using` fans out per declaring file and
never feeds tier (b), and both namespace declaration forms
(file-scoped and block) are read. `Ports/`/`Services/` additionally
exercise ADR-0029's type-position capture — the field/base-clause case
that motivated it (see that ADR's Context for the real report).
`TopLevelRegistration.cs` exercises ADR-0031's follow-up fix — a call
or type ref sitting in C# top-level-statement code, which has no
enclosing symbol at all.

| Path / call | Exercises |
|---|---|
| `Program.cs`'s `using Acme.Orders;` | `RawImport::NamespaceImport` — exact match against declared namespaces, fanning out to **two** `imports` edges, one per declaring file (`Orders/Order.cs`, `Orders/OrderParser.cs`), evidence `namespace-import` |
| `Program.cs`'s `using System;` / `using System.Text.Json;` | no walked file declares either namespace, roots unknown — external `Module` nodes keyed by the **full** namespace string (`System`, `System.Text.Json` — not truncated to a shared `System` root); `Orders/OrderParser.cs`'s own `using System;` dedupes onto the same `System` node |
| `Program.cs`'s `using Acme.Reports;` | known namespace root (`Acme`) but no file declares that exact namespace — internal-but-unresolvable, honest omission (INV-8): no edge, no node |
| `Program.cs`'s `using Parser = Acme.Orders.OrderParser;` | the alias form — `RawImport::Qualified` resolved through the FQN index (`.`-separated via `namespace_separator`), edge to `Orders/OrderParser.cs`; deduped with the fan-out edge to the same file |
| `Auditing/AuditLog.cs`'s `using static Acme.Orders.Order;` | `using static` names a *type* — also `Qualified`, edge to `Orders/Order.cs`; the member-binding behavior itself is not modeled |
| `Main` calling `new Parser()` | object creation as a call site **and** tier (b): the alias binds `Parser` → declared `OrderParser` (ADR-0013's alias machinery), evidence `imported` — the target class is `internal`, which counts as exported (ADR-0016) |
| `Parse` calling `new Order(...)` | object creation resolving cross-file via tier (c), evidence `same-package` — the constructed *type* is the target; `Order` has an explicit constructor, which is exactly why constructors aren't extracted as symbols |
| `Order`'s constructor calling `Stamp(id)` | a constructor body's calls are attributed to the enclosing *class* symbol (innermost containment — the constructor itself is not a symbol), resolving tier (a) `same-file` |
| `Parse` calling `Record(order)` (`using Acme.Auditing;`) | tier (c) `same-package` despite the genuine `using` — a plain namespace `using` never feeds tier (b) (binds no symbol name), C#'s version of Go's "never tier (b)" consequence |
| `Main` calling `parser.Parse(...)` / `order.Summary()` | member calls, rightmost-identifier capture, tier (c) |
| `Summary` calling `Describe` / `Record` calling `Flush` | tier (a) `same-file`, incl. a private (non-exported) target |
| `Main`'s `Console.WriteLine`, `Parse`'s `Guid.NewGuid()`, `Normalize`'s `raw.Trim()` | stdlib/receiver calls with no visible symbol — `unresolved_calls`, the same honesty principle as every other fixture |
| `Orders/Order.cs` | file-scoped namespace; `record` → `class`, `record struct` → `struct`, `enum`, `const` field (`Order.DefaultStatus`, one symbol) — and a non-const field (`Id`) that is deliberately **not** a symbol |
| `Auditing/AuditLog.cs` | *block* namespace form; `interface` (+ its modifier-less member `Write`, default-public), `delegate` → `type`, `static class` |
| `Services/QueryJobService.cs`'s field + constructor parameter of type `IQueryJobStore` | ADR-0029's acceptance case: two type refs to the same interface, deduped to **one** `references` edge (`type-reference:same-package` — a plain namespace `using` never feeds tier (b), same as a `calls` edge would get) |
| `Services/S3PresignedUrlProvider.cs`'s `: IPresignedUrlProvider` base clause | a `references` edge to `IPresignedUrlProvider`, **not** to `IQueryJobStore` — despite this file also `using`-ing `Acme.Ports` (the same namespace `IQueryJobStore` lives in) and therefore appearing in `IQueryJobStore.cs`'s `imports` fan-out too. This is the precision gap the field-report ADR-0029 responds to: `deps IQueryJobStore --dir in --depth 1 --kinds references` returns `QueryJobService` and `TopLevelRegistration.cs` (below) and nothing else; `deps Ports/IQueryJobStore.cs --dir in --depth 1 --kinds imports` (the old, only, path) honestly returns both service files, since both genuinely import the namespace |
| `TopLevelRegistration.cs`'s top-level-statement call `Registrar.Register<IQueryJobStore, QueryJobService>();` | ADR-0031's acceptance case: C# 9+ top-level statements have no enclosing method/class at all — before ADR-0031 this call/type-ref pair was silently dropped, invisible even as `unresolved_calls`, since nothing in `symbols.scm` captures "the top level of a file" as a container. Now resolves at *file* scope: a `calls` edge `TopLevelRegistration.cs → Register` (tier same-file, since `Registrar` is declared later in the same file) and two `references` edges, `TopLevelRegistration.cs → IQueryJobStore` / `→ QueryJobService` (tier same-package, cross-file) — all three sourced from the **File** node, not a Symbol, since none exists to attribute them to |

Verify with:

```bash
cargo run -p carto-cli -- index fixtures/csharp-app --out /tmp/carto-cs --json
python3 -m json.tool /tmp/carto-cs/graph.json
```
