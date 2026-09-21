using Acme.Ports;

namespace Acme.Services;

public sealed class S3PresignedUrlProvider : IPresignedUrlProvider
{
    public string Sign(string key)
    {
        return key;
    }
}
