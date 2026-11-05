using System.Collections.Generic;

namespace Acme.Billing
{
    public delegate void Paid(Invoice invoice);

    public interface IPriced
    {
        decimal Price();
    }

    public enum Status { Draft, Sent = 2 }

    public struct Money
    {
        public decimal Amount;
    }

    public record Line(string Label, decimal Amount);

    public class Invoice : IPriced
    {
        public const int MaxLines = 100;
        private readonly List<Line> lines = new();
        public event Paid OnPaid;

        public Invoice(List<Line> lines)
        {
            this.lines = lines;
        }

        ~Invoice() { }

        public decimal Total { get; private set; }

        internal string Label => "invoice";

        public decimal Price() => Total;

        private static Invoice Empty() { return new Invoice(new List<Line>()); }
    }
}
