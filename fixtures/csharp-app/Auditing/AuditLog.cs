using static Acme.Orders.Order;

namespace Acme.Auditing
{
    public interface IAuditSink
    {
        void Write(string entry);
    }

    public delegate void AuditHook(string entry);

    public static class AuditLog
    {
        public static void Record(object order)
        {
            Flush();
        }

        private static void Flush() { }
    }
}
