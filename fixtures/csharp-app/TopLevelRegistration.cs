using Acme.Ports;
using Acme.Services;

// C# 9+ top-level statements — the modern ASP.NET Core minimal-API
// `Program.cs` style, with no `Main` method at all. Exercises
// ADR-0031: before it, a call or type reference sitting in top-level-
// statement code (outside any method/class the extractor captures as
// a symbol) was silently dropped instead of resolved at file scope —
// exactly the DI-registration shape below
// (`services.AddSingleton<TService, TImpl>()`) that motivated it.
// `Registrar.Register` is a dependency-free stand-in for ASP.NET
// Core's real `IServiceCollection.AddSingleton<TService,
// TImplementation>()`.
//
// Note: a real C# project allows top-level statements in only one
// file, and this fixture already has `Program.cs` in that role — this
// file couldn't coexist with it in a project `csc` would actually
// compile. Irrelevant here: carto only parses, never compiles.
Registrar.Register<IQueryJobStore, QueryJobService>();

public static class Registrar
{
    public static void Register<TService, TImplementation>()
    {
    }

    // ADR-0032's negative case: unlike `QueryJobService.CancelQuery`
    // (Services/QueryJobService.cs), this method's own parameters name
    // *both* `Save` candidates' owner types in this same file — so the
    // owner-type filter has two survivors, not one, and `primary.Save`
    // stays exactly as ambiguous as it would without ADR-0032's tier at
    // all. This is also ADR-0033's acceptance case: both `Save` symbols
    // must record this call site in their own `unresolved_inbound_
    // calls`, not report a false all-clear.
    public static void RegisterQueryJobStore(IQueryJobStore primary, InMemoryQueryJobStore fallback)
    {
        primary.Save("noop");
    }
}
