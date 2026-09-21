namespace Acme.Billing;

// A third same-named Handler (see services/orders/handler.go's own
// comment) -- billing's own component, never called by orders or
// admin in this fixture, so it must never appear as a candidate for
// either of their bare Handler() calls.
public class Handler
{
    public string Handle()
    {
        return "billing";
    }
}
