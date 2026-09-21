using Acme.Ports;

namespace Acme.Services;

// ADR-0032's acceptance case, other half: the second `Save` declaration
// that makes `QueryJobService.CancelQuery`'s call ambiguous by bare
// name alone. Also exercises the same base-clause `references` capture
// `S3PresignedUrlProvider.cs` does — `: IQueryJobStore` resolves to
// `Ports/IQueryJobStore.cs`, same-package tier.
public sealed class InMemoryQueryJobStore : IQueryJobStore
{
    public void Save(string jobId)
    {
    }
}
