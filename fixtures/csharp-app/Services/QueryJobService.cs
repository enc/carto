using Acme.Ports;

namespace Acme.Services;

public sealed class QueryJobService
{
    private readonly IQueryJobStore _store;

    public QueryJobService(IQueryJobStore store)
    {
        _store = store;
    }

    // ADR-0032's acceptance case: `Save` is declared twice repo-wide
    // (here, on the interface, and again on InMemoryQueryJobStore.cs's
    // implementation) — an interface-mediated call through the
    // interface-typed `_store` field. Without owner-type
    // disambiguation this is a same-package tier ambiguity (two
    // same-named candidates) and would silently produce no edge; the
    // field's own `IQueryJobStore` type ref (constructor parameter,
    // above) is what narrows it to exactly one.
    public void CancelQuery(string jobId)
    {
        _store.Save(jobId);
    }
}
