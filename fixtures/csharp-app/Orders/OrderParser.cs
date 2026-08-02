using System;
using Acme.Auditing;

namespace Acme.Orders;

internal class OrderParser
{
    public Order Parse(string raw)
    {
        var order = new Order(Normalize(raw));
        Record(order);
        Guid.NewGuid();
        return order;
    }

    private static string Normalize(string raw)
    {
        return raw.Trim();
    }
}
