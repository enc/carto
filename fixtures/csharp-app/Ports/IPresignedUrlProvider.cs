namespace Acme.Ports;

public interface IPresignedUrlProvider
{
    string Sign(string key);
}
