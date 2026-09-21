namespace Acme.Ports;

public interface IQueryJobStore
{
    void Save(string jobId);
}
