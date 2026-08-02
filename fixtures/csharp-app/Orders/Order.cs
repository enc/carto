namespace Acme.Orders;

public record OrderLine(string Sku);

public record struct Money(decimal Amount);

public enum Status
{
    Open,
    Closed,
}

public class Order
{
    public const string DefaultStatus = "open";

    public string Id;

    public Order(string id)
    {
        Stamp(id);
    }

    public string Summary()
    {
        return Describe(Id);
    }

    private static string Describe(string id)
    {
        return "order " + id;
    }

    private static void Stamp(string id) { }
}
