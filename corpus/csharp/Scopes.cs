using System;
using System.Collections.Generic;
using Money = System.Decimal;

namespace Acme.Billing
{
    public class Cart<T> where T : IComparable<T>
    {
        private readonly List<T> items = new List<T>();
        private int count;

        public int Add(T item)
        {
            items.Add(item);
            this.count++;
            count = count + 1;
            return Total(count);
        }

        public int Total(int extra = 0)
        {
            var sum = extra;
            foreach (var item in items)
            {
                sum += 1;
            }
            Func<int, int> twice = x => x * 2;
            if (int.TryParse("1", out var parsed))
            {
                sum += parsed;
            }
            try
            {
                return twice(sum) + ToString().Length;
            }
            catch (Exception error)
            {
                throw new InvalidOperationException(error.Message);
            }
        }
    }

    public class Special : Cart<string>
    {
        public int Shout() => Total(extra: 2);
    }
}
