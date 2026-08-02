using System;
using System.Text.Json;
using Acme.Orders;
using Acme.Reports;
using Parser = Acme.Orders.OrderParser;

namespace Acme.App;

public class Program
{
    public static void Main()
    {
        var parser = new Parser();
        var order = parser.Parse("o-7");
        Console.WriteLine(order.Summary());
    }
}
